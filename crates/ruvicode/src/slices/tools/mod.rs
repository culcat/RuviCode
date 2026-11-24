use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use ruvi_core::{AgentTool, CoreError, CoreResult, ExtensionRegistry};
use serde_json::{Value, json};
use tokio::fs;

/// Resolves a workspace-relative path safely, preventing directory traversal outside `workspace_root`.
fn resolve_workspace_path(workspace_root: &Path, raw: &str) -> CoreResult<PathBuf> {
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace_root.join(candidate)
    };
    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let canonical_target = joined.canonicalize().map_err(|err| CoreError::Execution {
        component: "workspace_path".to_string(),
        message: format!("cannot resolve `{raw}`: {err}"),
    })?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(CoreError::Execution {
            component: "workspace_path".to_string(),
            message: format!("path `{raw}` escapes workspace root"),
        });
    }
    Ok(canonical_target)
}

/// Built-in [`AgentTool`] for reading a slice of a file inside the workspace.
pub struct ReadFileTool {
    workspace_root: PathBuf,
}

impl ReadFileTool {
    /// Creates a [`ReadFileTool`] rooted at `workspace_root`.
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

#[async_trait]
impl AgentTool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read UTF-8 text lines from a file inside the workspace"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Relative path to the file" },
                "start_line": { "type": "integer", "description": "1-based start line (optional)" },
                "end_line": { "type": "integer", "description": "1-based inclusive end line (optional)" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: Value) -> CoreResult<Value> {
        let path_str =
            input
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::Execution {
                    component: self.name().to_string(),
                    message: "missing string field `path`".to_string(),
                })?;
        let target = resolve_workspace_path(&self.workspace_root, path_str)?;
        let content = fs::read_to_string(&target).await?;

        let start = input
            .get("start_line")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(1)
            .max(1);
        let end = input
            .get("end_line")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(start + 199);

        let mut snippet = String::new();
        for (idx, line) in content.lines().enumerate() {
            let line_no = idx + 1;
            if line_no >= start && line_no <= end {
                let _ = writeln!(snippet, "{line_no}: {line}");
            }
        }

        Ok(json!({
            "path": path_str,
            "start_line": start,
            "end_line": end,
            "content": snippet
        }))
    }
}

/// Built-in [`AgentTool`] for listing files and subdirectories in the workspace.
pub struct ListDirTool {
    workspace_root: PathBuf,
}

impl ListDirTool {
    /// Creates a [`ListDirTool`] rooted at `workspace_root`.
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

#[async_trait]
impl AgentTool for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn description(&self) -> &'static str {
        "List files and subdirectories in a workspace directory"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Relative directory path (defaults to `.`)" }
            }
        })
    }

    async fn execute(&self, input: Value) -> CoreResult<Value> {
        let rel = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let dir = resolve_workspace_path(&self.workspace_root, rel)?;

        let mut entries = fs::read_dir(&dir).await?;
        let mut items = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(name.as_str(), ".git" | "target") {
                continue;
            }
            let kind = if entry.path().is_dir() { "dir" } else { "file" };
            items.push(json!({ "name": name, "type": kind }));
        }
        items.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

        Ok(json!({ "path": rel, "entries": items }))
    }
}

/// Registers built-in workspace inspection tools into [`ExtensionRegistry`].
pub fn register_builtin_tools(registry: &mut ExtensionRegistry, workspace_root: &Path) {
    registry.register_tool(Arc::new(ReadFileTool::new(workspace_root)));
    registry.register_tool(Arc::new(ListDirTool::new(workspace_root)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn builtin_tools_list_and_read_workspace_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");

        let list_tool = ListDirTool::new(&root);
        let listed = list_tool
            .execute(json!({ "path": "." }))
            .await
            .expect("list_dir should succeed");
        let entries = listed["entries"].as_array().expect("entries array");
        assert!(entries.iter().any(|e| e["name"] == "Cargo.toml"));

        let read_tool = ReadFileTool::new(&root);
        let read = read_tool
            .execute(json!({ "path": "agent.md", "start_line": 1, "end_line": 5 }))
            .await
            .expect("read_file should succeed");
        assert!(
            read["content"]
                .as_str()
                .is_some_and(|s| s.contains("RuviCode"))
        );
    }

    #[tokio::test]
    async fn builtin_tools_block_path_traversal() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");
        let read_tool = ReadFileTool::new(&root);
        let err = read_tool
            .execute(json!({ "path": "/etc/passwd" }))
            .await
            .expect_err("path outside workspace must be rejected");
        assert!(matches!(err, CoreError::Execution { .. }));
    }
}
