use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use ruvi_core::{AgentTool, CoreError, CoreResult, PluginHost, PluginManifest};
use serde_json::{Value, json};
use wasmi::{Engine, ExternType, Instance, Linker, Module, Store, ValType};

#[derive(Clone, Debug)]
enum WasmAbi {
    V1 { entrypoint: String },
    V2,
}

fn detect_abi(module: &Module, component: &str) -> CoreResult<WasmAbi> {
    let has_memory = matches!(module.get_export("memory"), Some(ExternType::Memory(_)));
    let has_alloc = matches!(
        module.get_export("ruvi_alloc"),
        Some(ExternType::Func(func_ty))
            if func_ty.params() == [ValType::I32] && func_ty.results() == [ValType::I32]
    );
    let has_run_json = matches!(
        module.get_export("ruvi_run_json"),
        Some(ExternType::Func(func_ty))
            if func_ty.params() == [ValType::I32, ValType::I32]
                && func_ty.results() == [ValType::I64]
    );

    if has_memory && has_alloc && has_run_json {
        return Ok(WasmAbi::V2);
    }

    let entrypoint = ["ruvi_run", "run"]
        .into_iter()
        .find(|name| module.get_export(name).is_some())
        .ok_or_else(|| CoreError::Execution {
            component: component.to_string(),
            message: "WASM plugin must export ABI v2 (`memory`, `ruvi_alloc`, `ruvi_run_json`) or ABI v1 (`ruvi_run` / `run`)".to_string(),
        })?
        .to_string();

    Ok(WasmAbi::V1 { entrypoint })
}

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

        let abi = detect_abi(&module, &stem)?;

        let manifest = PluginManifest {
            id: stem.clone(),
            name: stem.clone(),
            version: "0.1.0".to_string(),
            capabilities: vec!["tool".to_string(), "wasm".to_string()],
            source: path.display().to_string(),
        };

        let tool = Arc::new(WasmPluginTool {
            name: stem,
            abi,
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
    abi: WasmAbi,
    bytes: Vec<u8>,
    engine: Engine,
    source: String,
}

fn execute_v1(
    instance: Instance,
    store: &mut Store<()>,
    entrypoint: &str,
    input: &Value,
    name: &str,
    source: &str,
) -> CoreResult<Value> {
    let arg = input
        .get("arg")
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(0);

    let exit_code = if let Ok(func) = instance.get_typed_func::<i32, i32>(&*store, entrypoint) {
        func.call(&mut *store, arg)
            .map_err(|err| CoreError::Execution {
                component: name.to_string(),
                message: err.to_string(),
            })?
    } else if let Ok(func) = instance.get_typed_func::<(), i32>(&*store, entrypoint) {
        func.call(&mut *store, ())
            .map_err(|err| CoreError::Execution {
                component: name.to_string(),
                message: err.to_string(),
            })?
    } else {
        return Err(CoreError::Execution {
            component: name.to_string(),
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
}

fn execute_v2(
    instance: Instance,
    store: &mut Store<()>,
    input: &Value,
    name: &str,
) -> CoreResult<Value> {
    let memory = instance
        .get_memory(&*store, "memory")
        .ok_or_else(|| CoreError::Execution {
            component: name.to_string(),
            message: "ABI v2 plugin missing exported `memory`".to_string(),
        })?;
    let alloc_fn = instance
        .get_typed_func::<i32, i32>(&*store, "ruvi_alloc")
        .map_err(|err| CoreError::Execution {
            component: name.to_string(),
            message: format!("ABI v2 plugin missing `ruvi_alloc(i32) -> i32`: {err}"),
        })?;
    let run_json_fn = instance
        .get_typed_func::<(i32, i32), i64>(&*store, "ruvi_run_json")
        .map_err(|err| CoreError::Execution {
            component: name.to_string(),
            message: format!("ABI v2 plugin missing `ruvi_run_json(i32, i32) -> i64`: {err}"),
        })?;

    let input_bytes = serde_json::to_vec(input).map_err(|err| CoreError::Execution {
        component: name.to_string(),
        message: format!("failed to serialize ABI v2 input JSON: {err}"),
    })?;
    let input_len = i32::try_from(input_bytes.len()).map_err(|_| CoreError::Execution {
        component: name.to_string(),
        message: "ABI v2 input JSON exceeds i32::MAX bytes".to_string(),
    })?;

    let in_ptr_i32 = alloc_fn
        .call(&mut *store, input_len)
        .map_err(|err| CoreError::Execution {
            component: name.to_string(),
            message: format!("`ruvi_alloc` trapped: {err}"),
        })?;
    let in_ptr = usize::try_from(in_ptr_i32).map_err(|_| CoreError::Execution {
        component: name.to_string(),
        message: format!("`ruvi_alloc` returned negative pointer: {in_ptr_i32}"),
    })?;

    let mem_size = memory.data_size(&*store);
    let in_end = in_ptr
        .checked_add(input_bytes.len())
        .ok_or_else(|| CoreError::Execution {
            component: name.to_string(),
            message: "ABI v2 input pointer overflow".to_string(),
        })?;
    if in_end > mem_size {
        return Err(CoreError::Execution {
            component: name.to_string(),
            message: format!(
                "ABI v2 input slice [{in_ptr}..{in_end}) exceeds WASM memory size {mem_size}"
            ),
        });
    }

    memory
        .write(&mut *store, in_ptr, &input_bytes)
        .map_err(|err| CoreError::Execution {
            component: name.to_string(),
            message: format!("failed to write ABI v2 input to WASM memory: {err}"),
        })?;

    let packed = run_json_fn
        .call(&mut *store, (in_ptr_i32, input_len))
        .map_err(|err| CoreError::Execution {
            component: name.to_string(),
            message: format!("`ruvi_run_json` trapped: {err}"),
        })?;

    let packed_u64 = u64::from_le_bytes(packed.to_le_bytes());
    let out_ptr = usize::try_from(packed_u64 >> 32).map_err(|_| CoreError::Execution {
        component: name.to_string(),
        message: "ABI v2 output pointer exceeds addressable range".to_string(),
    })?;
    let out_len = usize::try_from(packed_u64 & 0xffff_ffff).map_err(|_| CoreError::Execution {
        component: name.to_string(),
        message: "ABI v2 output length exceeds addressable range".to_string(),
    })?;

    let mem_size = memory.data_size(&*store);
    let out_end = out_ptr
        .checked_add(out_len)
        .ok_or_else(|| CoreError::Execution {
            component: name.to_string(),
            message: "ABI v2 output pointer overflow".to_string(),
        })?;
    if out_end > mem_size {
        return Err(CoreError::Execution {
            component: name.to_string(),
            message: format!(
                "ABI v2 output slice [{out_ptr}..{out_end}) exceeds WASM memory size {mem_size}"
            ),
        });
    }

    let mut out_bytes = vec![0u8; out_len];
    memory
        .read(&*store, out_ptr, &mut out_bytes)
        .map_err(|err| CoreError::Execution {
            component: name.to_string(),
            message: format!("failed to read ABI v2 output from WASM memory: {err}"),
        })?;

    serde_json::from_slice(&out_bytes).map_err(|err| CoreError::Execution {
        component: name.to_string(),
        message: format!("ABI v2 plugin returned invalid JSON: {err}"),
    })
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
        match &self.abi {
            WasmAbi::V1 { .. } => json!({
                "type": "object",
                "properties": {
                    "arg": { "type": "integer", "description": "Optional integer argument passed to `ruvi_run(i32)`" }
                }
            }),
            WasmAbi::V2 => json!({
                "type": "object",
                "additionalProperties": true,
                "description": "JSON input payload passed to `ruvi_run_json(ptr, len)`"
            }),
        }
    }

    async fn execute(&self, input: Value) -> CoreResult<Value> {
        let engine = self.engine.clone();
        let bytes = self.bytes.clone();
        let name = self.name.clone();
        let abi = self.abi.clone();
        let source = self.source.clone();

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

            match &abi {
                WasmAbi::V1 { entrypoint } => {
                    execute_v1(instance, &mut store, entrypoint, &input, &name, &source)
                }
                WasmAbi::V2 => execute_v2(instance, &mut store, &input, &name),
            }
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

    async fn load_single_wat_tool(
        name: &str,
        wat_src: &str,
    ) -> (std::path::PathBuf, ExtensionRegistry) {
        let temp_dir = std::env::temp_dir().join(format!(
            "ruvi-wasm-plugin-test-{}",
            ruvi_core::SessionId::new()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp plugin dir should be created");

        let wat_path = temp_dir.join(format!("{name}.wat"));
        std::fs::write(&wat_path, wat_src).expect("wat file should be written");

        let host = WasmiPluginHost::new();
        let (manifests, tools) = host
            .load_plugins(&[temp_dir.as_path()])
            .await
            .expect("wasm plugin should load");
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].id, name);

        let mut registry = ExtensionRegistry::new();
        registry.extend_plugin_manifests(manifests);
        for tool in tools {
            registry.register_tool(tool);
        }

        (temp_dir, registry)
    }

    #[tokio::test]
    async fn loads_and_executes_abi_v2_wat_plugin_via_registry() {
        let wat = r#"(module
            (memory (export "memory") 1)
            (global $heap (mut i32) (i32.const 1024))
            (data (i32.const 64) "{\"ok\":true,\"abi\":\"v2\"}")
            (func (export "ruvi_alloc") (param $len i32) (result i32)
                (local $ptr i32)
                (local.set $ptr (global.get $heap))
                (global.set $heap (i32.add (global.get $heap) (local.get $len)))
                (local.get $ptr))
            (func (export "ruvi_run_json") (param $ptr i32) (param $len i32) (result i64)
                (if (result i64)
                    (i32.and
                        (i32.gt_s (local.get $len) (i32.const 0))
                        (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 123)))
                    (then
                        (i64.or
                            (i64.shl (i64.const 64) (i64.const 32))
                            (i64.const 22)))
                    (else
                        (i64.or
                            (i64.shl (i64.const 999999) (i64.const 32))
                            (i64.const 4))))))"#;

        let (temp_dir, registry) = load_single_wat_tool("json_echo", wat).await;
        let output = registry
            .tool("json_echo")
            .expect("json_echo tool should be registered")
            .execute(json!({ "query": "rust" }))
            .await
            .expect("ABI v2 wasm execution should succeed");

        assert_eq!(output, json!({ "ok": true, "abi": "v2" }));
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn abi_v2_rejects_out_of_bounds_output() {
        let wat = r#"(module
            (memory (export "memory") 1)
            (func (export "ruvi_alloc") (param i32) (result i32)
                (i32.const 64))
            (func (export "ruvi_run_json") (param i32) (param i32) (result i64)
                (i64.or (i64.shl (i64.const 100000) (i64.const 32)) (i64.const 16))))"#;

        let (temp_dir, registry) = load_single_wat_tool("oob_plugin", wat).await;
        let err = registry
            .tool("oob_plugin")
            .expect("oob_plugin should be registered")
            .execute(json!({ "hello": "world" }))
            .await
            .expect_err("out-of-bounds output slice should fail");

        assert!(
            matches!(err, CoreError::Execution { .. }),
            "expected CoreError::Execution, got {err:?}"
        );
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn abi_v2_rejects_invalid_json_output() {
        let wat = r#"(module
            (memory (export "memory") 1)
            (data (i32.const 64) "not-json")
            (func (export "ruvi_alloc") (param i32) (result i32)
                (i32.const 1024))
            (func (export "ruvi_run_json") (param i32) (param i32) (result i64)
                (i64.or (i64.shl (i64.const 64) (i64.const 32)) (i64.const 8))))"#;

        let (temp_dir, registry) = load_single_wat_tool("bad_json", wat).await;
        let err = registry
            .tool("bad_json")
            .expect("bad_json should be registered")
            .execute(json!({ "hello": "world" }))
            .await
            .expect_err("invalid JSON output should fail");

        assert!(
            matches!(err, CoreError::Execution { .. }),
            "expected CoreError::Execution, got {err:?}"
        );
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
