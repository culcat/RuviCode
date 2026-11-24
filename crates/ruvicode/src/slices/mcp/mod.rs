use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use ruvi_core::{AgentTool, CoreError, CoreResult, ExtensionRegistry, McpServerConfig};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

const MCP_TIMEOUT: Duration = Duration::from_secs(4);

struct StdioTransport {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Live stdio JSON-RPC 2.0 client for a Model Context Protocol server.
pub struct McpStdioClient {
    server_name: String,
    transport: Mutex<StdioTransport>,
    next_id: AtomicU64,
}

/// Tool metadata returned by an MCP server's `tools/list` response.
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDescriptor {
    /// Tool identifier exposed by the MCP server.
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: String,
    /// JSON Schema for tool arguments.
    #[serde(rename = "inputSchema", default = "default_schema")]
    pub input_schema: Value,
}

fn default_schema() -> Value {
    json!({ "type": "object" })
}

impl McpStdioClient {
    /// Spawns a stdio MCP server process and completes the `initialize` handshake.
    ///
    /// # Errors
    /// Returns [`CoreError::Config`] for non-stdio targets or [`CoreError::Execution`] on handshake failure.
    pub async fn connect(config: &McpServerConfig) -> CoreResult<Self> {
        if config.target.starts_with("http://") || config.target.starts_with("https://") {
            return Err(CoreError::Config(format!(
                "MCP server `{}` uses HTTP URL `{}`; only stdio transport is active in this client",
                config.name, config.target
            )));
        }

        let mut child = Command::new(&config.target)
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| CoreError::Execution {
                component: format!("mcp:{}", config.name),
                message: format!("failed to spawn `{}`: {err}", config.target),
            })?;

        let stdin = child.stdin.take().ok_or_else(|| CoreError::Execution {
            component: format!("mcp:{}", config.name),
            message: "child stdin unavailable".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| CoreError::Execution {
            component: format!("mcp:{}", config.name),
            message: "child stdout unavailable".to_string(),
        })?;

        let client = Self {
            server_name: config.name.clone(),
            transport: Mutex::new(StdioTransport {
                _child: child,
                stdin,
                stdout: BufReader::new(stdout),
            }),
            next_id: AtomicU64::new(1),
        };

        let _ = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "ruvicode", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await?;

        client
            .notify("notifications/initialized", json!({}))
            .await?;
        Ok(client)
    }

    /// Queries the server's available tools via `tools/list`.
    ///
    /// # Errors
    /// Returns [`CoreError::Execution`] if the request fails or returns invalid JSON.
    pub async fn list_tools(&self) -> CoreResult<Vec<McpToolDescriptor>> {
        let result = self.request("tools/list", json!({})).await?;
        let tools_val = result.get("tools").cloned().unwrap_or_else(|| json!([]));
        serde_json::from_value(tools_val).map_err(|err| CoreError::Execution {
            component: format!("mcp:{}", self.server_name),
            message: format!("invalid tools/list payload: {err}"),
        })
    }

    /// Invokes a named tool on the MCP server via `tools/call`.
    ///
    /// # Errors
    /// Returns [`CoreError::Execution`] if the RPC call fails.
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> CoreResult<Value> {
        self.request(
            "tools/call",
            json!({
                "name": tool_name,
                "arguments": arguments
            }),
        )
        .await
    }

    async fn notify(&self, method: &str, params: Value) -> CoreResult<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        let mut line = msg.to_string();
        line.push('\n');

        let mut guard = self.transport.lock().await;
        guard
            .stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|err| CoreError::Execution {
                component: format!("mcp:{}", self.server_name),
                message: err.to_string(),
            })?;
        guard
            .stdin
            .flush()
            .await
            .map_err(|err| CoreError::Execution {
                component: format!("mcp:{}", self.server_name),
                message: err.to_string(),
            })
    }

    async fn request(&self, method: &str, params: Value) -> CoreResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let mut line = msg.to_string();
        line.push('\n');

        let mut guard = self.transport.lock().await;
        guard
            .stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|err| CoreError::Execution {
                component: format!("mcp:{}", self.server_name),
                message: err.to_string(),
            })?;
        guard
            .stdin
            .flush()
            .await
            .map_err(|err| CoreError::Execution {
                component: format!("mcp:{}", self.server_name),
                message: err.to_string(),
            })?;

        let read_fut = async {
            let mut response_line = String::new();
            loop {
                response_line.clear();
                let n = guard.stdout.read_line(&mut response_line).await?;
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "MCP server closed stdout",
                    ));
                }
                let trimmed = response_line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(parsed) = serde_json::from_str::<Value>(trimmed)
                    && parsed.get("id").and_then(Value::as_u64) == Some(id)
                {
                    return Ok(parsed);
                }
            }
        };

        let parsed = tokio::time::timeout(MCP_TIMEOUT, read_fut)
            .await
            .map_err(|_| CoreError::Execution {
                component: format!("mcp:{}", self.server_name),
                message: format!("timed out waiting for `{method}` response"),
            })?
            .map_err(|err| CoreError::Execution {
                component: format!("mcp:{}", self.server_name),
                message: err.to_string(),
            })?;

        if let Some(err_obj) = parsed.get("error") {
            return Err(CoreError::Execution {
                component: format!("mcp:{}", self.server_name),
                message: format!("JSON-RPC error on `{method}`: {err_obj}"),
            });
        }

        Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// Wraps a single tool exposed by a live [`McpStdioClient`] as an [`AgentTool`].
pub struct McpAgentTool {
    registered_name: String,
    remote_name: String,
    description: String,
    schema: Value,
    client: Arc<McpStdioClient>,
}

#[async_trait]
impl AgentTool for McpAgentTool {
    fn name(&self) -> &str {
        &self.registered_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    async fn execute(&self, input: Value) -> CoreResult<Value> {
        self.client.call_tool(&self.remote_name, input).await
    }
}

/// Connects to configured stdio MCP servers and registers their tools into [`ExtensionRegistry`].
pub async fn register_mcp_tools(
    registry: &mut ExtensionRegistry,
    servers: &[McpServerConfig],
) -> Vec<String> {
    let mut warnings = Vec::new();
    for server in servers {
        match McpStdioClient::connect(server).await {
            Ok(client) => {
                let client = Arc::new(client);
                match client.list_tools().await {
                    Ok(tools) => {
                        for desc in tools {
                            let registered_name = format!("mcp__{}__{}", server.name, desc.name);
                            registry.register_tool(Arc::new(McpAgentTool {
                                registered_name,
                                remote_name: desc.name,
                                description: desc.description,
                                schema: desc.input_schema,
                                client: Arc::clone(&client),
                            }));
                        }
                    }
                    Err(err) => warnings.push(format!("{}: {err}", server.name)),
                }
            }
            Err(err) => {
                tracing::warn!(server = %server.name, "skipping MCP server: {err}");
                warnings.push(format!("{}: {err}", server.name));
            }
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connects_to_stdio_mcp_server_and_executes_tool() {
        let script = r#"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{}}}\n'
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"ping","description":"Returns pong","inputSchema":{"type":"object"}}]}}\n'
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"pong"}]}}\n'
      ;;
  esac
done
"#;

        let config = McpServerConfig {
            name: "mock".to_string(),
            target: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            source_file: "test".to_string(),
        };

        let mut registry = ExtensionRegistry::new();
        let warnings = register_mcp_tools(&mut registry, &[config]).await;
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let output = registry
            .tool("mcp__mock__ping")
            .expect("MCP tool should be registered")
            .execute(json!({}))
            .await
            .expect("MCP tool call should succeed");

        assert_eq!(output["content"][0]["text"], "pong");
    }
}
