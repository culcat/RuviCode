use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use ruvi_core::{AgentTool, CoreError, CoreResult, PluginHost, PluginManifest};
use serde_json::{Value, json};
use wasmi::{Engine, Linker, Module, Store};

/// Discovers and loads sandboxed `.wasm` and `.wat` user plugins using `wasmi`.
pub struct WasmiPluginHost {
    engine: Engine,
}

impl WasmiPluginHost {
    /// Creates a new [`WasmiPluginHost`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            engine: Engine::default(),
        }
    }

    fn load_plugin_file(&self, path: &Path) -> CoreResult<(PluginManifest, Arc<dyn AgentTool>)> {
        let bytes = std::fs::read(path)?;
        let module = Module::new(&self.engine, &bytes[..]).map_err(|err| CoreError::Execution {
            component: path.display().to_string(),
            message: format!("failed to compile WASM module: {err}"),
        })?;

        let stem = path
            .file_stem()
            .map_or_else(|| "plugin".to_string(), |s| s.to_string_lossy().to_string());

        let entrypoint = ["ruvi_run", "run"]
            .into_iter()
            .find(|name| module.get_export(name).is_some())
            .ok_or_else(|| CoreError::Execution {
                component: stem.clone(),
                message: "WASM plugin must export `ruvi_run` or `run`".to_string(),
            })?
            .to_string();

        let manifest = PluginManifest {
            id: stem.clone(),
            name: stem.clone(),
            version: "0.1.0".to_string(),
            capabilities: vec!["tool".to_string(), "wasm".to_string()],
            source: path.display().to_string(),
        };

        let tool = Arc::new(WasmPluginTool {
            name: stem,
            entrypoint,
            bytes,
            engine: self.engine.clone(),
            source: path.display().to_string(),
        });

        Ok((manifest, tool))
    }
}

impl Default for WasmiPluginHost {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PluginHost for WasmiPluginHost {
    fn id(&self) -> &'static str {
        "wasmi"
    }

    async fn load_plugins(
        &self,
        plugin_dirs: &[&Path],
    ) -> CoreResult<(Vec<PluginManifest>, Vec<Arc<dyn AgentTool>>)> {
        let mut manifests = Vec::new();
        let mut tools = Vec::new();

        for dir in plugin_dirs {
            if !dir.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(dir)? {
                let path = entry?.path();
                if !path.is_file() {
                    continue;
                }
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default();
                if !matches!(ext, "wasm" | "wat") {
                    continue;
                }
                let (manifest, tool) = self.load_plugin_file(&path)?;
                manifests.push(manifest);
                tools.push(tool);
            }
        }

        Ok((manifests, tools))
    }
}

/// Exposes a sandboxed WASM module's exported function as an [`AgentTool`].
pub struct WasmPluginTool {
    name: String,
    entrypoint: String,
    bytes: Vec<u8>,
    engine: Engine,
    source: String,
}

#[async_trait]
impl AgentTool for WasmPluginTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &'static str {
        "Sandboxed WASM plugin tool"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "arg": { "type": "integer", "description": "Optional integer argument passed to `ruvi_run(i32)`" }
            }
        })
    }

    async fn execute(&self, input: Value) -> CoreResult<Value> {
        let engine = self.engine.clone();
        let bytes = self.bytes.clone();
        let name = self.name.clone();
        let entrypoint = self.entrypoint.clone();
        let source = self.source.clone();
        let arg = input
            .get("arg")
            .and_then(Value::as_i64)
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or(0);

        tokio::task::spawn_blocking(move || -> CoreResult<Value> {
            let module = Module::new(&engine, &bytes[..]).map_err(|err| CoreError::Execution {
                component: name.clone(),
                message: err.to_string(),
            })?;
            let mut store = Store::new(&engine, ());
            let linker = Linker::<()>::new(&engine);
            let instance = linker
                .instantiate_and_start(&mut store, &module)
                .map_err(|err| CoreError::Execution {
                    component: name.clone(),
                    message: err.to_string(),
                })?;

            let exit_code = if let Ok(func) =
                instance.get_typed_func::<i32, i32>(&store, &entrypoint)
            {
                func.call(&mut store, arg)
                    .map_err(|err| CoreError::Execution {
                        component: name.clone(),
                        message: err.to_string(),
                    })?
            } else if let Ok(func) = instance.get_typed_func::<(), i32>(&store, &entrypoint) {
                func.call(&mut store, ())
                    .map_err(|err| CoreError::Execution {
                        component: name.clone(),
                        message: err.to_string(),
                    })?
            } else {
                return Err(CoreError::Execution {
                    component: name,
                    message: format!(
                        "exported function `{entrypoint}` must have signature `() -> i32` or `(i32) -> i32`"
                    ),
                });
            };

            Ok(json!({
                "plugin": name,
                "source": source,
                "exit_code": exit_code
            }))
        })
        .await
        .map_err(|err| CoreError::Execution {
            component: self.name.clone(),
            message: err.to_string(),
        })?
    }
}

#[cfg(test)]
mod tests {
    use ruvi_core::ExtensionRegistry;

    use super::*;

    #[tokio::test]
    async fn loads_and_executes_wat_plugin_via_registry() {
        let temp_dir = std::env::temp_dir().join(format!(
            "ruvi-wasm-plugin-test-{}",
            ruvi_core::SessionId::new()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp plugin dir should be created");

        let wat_path = temp_dir.join("adder.wat");
        std::fs::write(
            &wat_path,
            r#"(module (func (export "ruvi_run") (param i32) (result i32) local.get 0 i32.const 7 i32.add))"#,
        )
        .expect("wat file should be written");

        let host = WasmiPluginHost::new();
        let (manifests, tools) = host
            .load_plugins(&[temp_dir.as_path()])
            .await
            .expect("wasm plugin should load");
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].id, "adder");

        let mut registry = ExtensionRegistry::new();
        registry.extend_plugin_manifests(manifests);
        for tool in tools {
            registry.register_tool(tool);
        }

        let output = registry
            .tool("adder")
            .expect("adder tool should be registered")
            .execute(json!({ "arg": 35 }))
            .await
            .expect("wasm execution should succeed");

        assert_eq!(output["exit_code"], 42);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
