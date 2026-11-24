use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ruvi_core::{AgentTool, CoreError, CoreResult, ExtensionRegistry};
use serde_json::{Value, json};
use tokio::fs;
use tokio::process::Command;

const MAX_GREP_MATCHES: usize = 50;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

fn required_str<'a>(input: &'a Value, field: &str, component: &str) -> CoreResult<&'a str> {
    input
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Execution {
            component: component.to_string(),
            message: format!("missing string field `{field}`"),
        })
}

/// Resolves an existing workspace path safely, preventing directory traversal outside `workspace_root`.
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

/// Resolves a writable workspace file path safely before creating parent directories.
///
/// Rejects `..` components escaping `workspace_root` and canonicalizes the nearest
/// existing ancestor to prevent symlink escapes prior to `create_dir_all`.
fn resolve_writable_workspace_path(workspace_root: &Path, raw: &str) -> CoreResult<PathBuf> {
    if raw.trim().is_empty() || raw.ends_with('/') || raw.ends_with('\\') {
        return Err(CoreError::Execution {
            component: "workspace_path".to_string(),
            message: format!("invalid file path `{raw}`"),
        });
    }

    let candidate = Path::new(raw);
    if matches!(
        candidate.components().next_back(),
        Some(Component::CurDir) | None
    ) {
        return Err(CoreError::Execution {
            component: "workspace_path".to_string(),
            message: format!("invalid file path `{raw}`"),
        });
    }

    let joined = if candidate.is_absolute() {
        if candidate
            .components()
            .any(|comp| matches!(comp, Component::ParentDir))
        {
            return Err(CoreError::Execution {
                component: "workspace_path".to_string(),
                message: format!("path `{raw}` escapes workspace root"),
            });
        }
        candidate.to_path_buf()
    } else {
        let mut normalized = PathBuf::new();
        for comp in candidate.components() {
            match comp {
                Component::CurDir => {}
                Component::Normal(seg) => normalized.push(seg),
                Component::ParentDir => {
                    if !normalized.pop() {
                        return Err(CoreError::Execution {
                            component: "workspace_path".to_string(),
                            message: format!("path `{raw}` escapes workspace root"),
                        });
                    }
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(CoreError::Execution {
                        component: "workspace_path".to_string(),
                        message: format!("invalid path component in `{raw}`"),
                    });
                }
            }
        }
        if normalized.as_os_str().is_empty() {
            return Err(CoreError::Execution {
                component: "workspace_path".to_string(),
                message: format!("invalid file path `{raw}`"),
            });
        }
        workspace_root.join(normalized)
    };

    let canonical_root = workspace_root
        .canonicalize()
        .map_err(|err| CoreError::Execution {
            component: "workspace_path".to_string(),
            message: format!("cannot resolve workspace root: {err}"),
        })?;

    for ancestor in joined.ancestors() {
        if ancestor.as_os_str().is_empty() {
            break;
        }
        if ancestor.symlink_metadata().is_ok() {
            let canonical_ancestor =
                ancestor
                    .canonicalize()
                    .map_err(|err| CoreError::Execution {
                        component: "workspace_path".to_string(),
                        message: format!("cannot resolve `{raw}`: {err}"),
                    })?;
            if !canonical_ancestor.starts_with(&canonical_root) {
                return Err(CoreError::Execution {
                    component: "workspace_path".to_string(),
                    message: format!("path `{raw}` escapes workspace root"),
                });
            }
            let suffix = joined
                .strip_prefix(ancestor)
                .map_err(|err| CoreError::Execution {
                    component: "workspace_path".to_string(),
                    message: format!("cannot compute relative path for `{raw}`: {err}"),
                })?;
            return Ok(if suffix.as_os_str().is_empty() {
                canonical_ancestor
            } else {
                canonical_ancestor.join(suffix)
            });
        }
    }

    Err(CoreError::Execution {
        component: "workspace_path".to_string(),
        message: format!("cannot resolve `{raw}` within workspace root"),
    })
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
        let path_str = required_str(&input, "path", self.name())?;
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

async fn search_single_file(
    canonical_root: &Path,
    file_path: &Path,
    query: &str,
    matches: &mut Vec<Value>,
    truncated: &mut bool,
) {
    let Ok(bytes) = fs::read(file_path).await else {
        return;
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return;
    };
    let rel_path = file_path
        .strip_prefix(canonical_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .into_owned();

    for (idx, line) in text.lines().enumerate() {
        if line.contains(query) {
            if matches.len() >= MAX_GREP_MATCHES {
                *truncated = true;
                return;
            }
            matches.push(json!({
                "path": rel_path,
                "line": idx + 1,
                "text": line,
            }));
        }
    }
}

async fn collect_grep_matches(
    canonical_root: &Path,
    search_root: &Path,
    query: &str,
) -> CoreResult<(Vec<Value>, bool)> {
    let mut matches = Vec::new();
    let mut truncated = false;

    if search_root.is_file() {
        search_single_file(
            canonical_root,
            search_root,
            query,
            &mut matches,
            &mut truncated,
        )
        .await;
        return Ok((matches, truncated));
    }

    let mut stack = vec![search_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = fs::read_dir(&dir).await?;
        let mut children = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            if matches!(name.to_str(), Some(".git" | "target")) {
                continue;
            }
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            children.push((name, entry.path(), file_type.is_dir(), file_type.is_file()));
        }
        children.sort_by(|a, b| a.0.cmp(&b.0));

        let mut subdirs = Vec::new();
        for (_name, path, is_dir, is_file) in children {
            if is_dir {
                subdirs.push(path);
            } else if is_file {
                search_single_file(canonical_root, &path, query, &mut matches, &mut truncated)
                    .await;
                if truncated {
                    return Ok((matches, true));
                }
            }
        }
        for subdir in subdirs.into_iter().rev() {
            stack.push(subdir);
        }
    }

    Ok((matches, truncated))
}

/// Built-in [`AgentTool`] for recursively searching UTF-8 workspace files for a text query.
pub struct GrepSearchTool {
    workspace_root: PathBuf,
}

impl GrepSearchTool {
    /// Creates a [`GrepSearchTool`] rooted at `workspace_root`.
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

#[async_trait]
impl AgentTool for GrepSearchTool {
    fn name(&self) -> &'static str {
        "grep_search"
    }

    fn description(&self) -> &'static str {
        "Recursively search UTF-8 workspace files for a substring"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Non-empty substring to search for" },
                "path": { "type": "string", "description": "Optional relative subpath to search (defaults to `.`)" }
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> CoreResult<Value> {
        let query = required_str(&input, "query", self.name())?;
        if query.is_empty() {
            return Err(CoreError::Execution {
                component: self.name().to_string(),
                message: "field `query` must not be empty".to_string(),
            });
        }

        let rel = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let search_root = resolve_workspace_path(&self.workspace_root, rel)?;
        let canonical_root = self
            .workspace_root
            .canonicalize()
            .unwrap_or_else(|_| self.workspace_root.clone());

        let (matches, truncated) =
            collect_grep_matches(&canonical_root, &search_root, query).await?;
        Ok(json!({
            "query": query,
            "matches": matches,
            "truncated": truncated,
        }))
    }
}

/// Built-in [`AgentTool`] for creating or overwriting a file inside the workspace.
pub struct WriteFileTool {
    workspace_root: PathBuf,
}

impl WriteFileTool {
    /// Creates a [`WriteFileTool`] rooted at `workspace_root`.
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

#[async_trait]
impl AgentTool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Create or overwrite a UTF-8 file inside the workspace"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Relative path to the file" },
                "content": { "type": "string", "description": "UTF-8 content to write" }
            },
            "required": ["path", "content"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> CoreResult<Value> {
        let path_str = required_str(&input, "path", self.name())?;
        let content = required_str(&input, "content", self.name())?;
        let target = resolve_writable_workspace_path(&self.workspace_root, path_str)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&target, content.as_bytes()).await?;

        Ok(json!({
            "path": path_str,
            "bytes_written": content.len(),
        }))
    }
}

/// Built-in [`AgentTool`] for replacing an exact substring inside an existing workspace file.
pub struct EditFileTool {
    workspace_root: PathBuf,
}

impl EditFileTool {
    /// Creates an [`EditFileTool`] rooted at `workspace_root`.
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

#[async_trait]
impl AgentTool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Replace an exact target snippet inside an existing workspace file"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Relative path to the existing file" },
                "target": { "type": "string", "description": "Non-empty exact text to replace" },
                "replacement": { "type": "string", "description": "Replacement text" }
            },
            "required": ["path", "target", "replacement"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> CoreResult<Value> {
        let path_str = required_str(&input, "path", self.name())?;
        let target_text = required_str(&input, "target", self.name())?;
        let replacement = required_str(&input, "replacement", self.name())?;
        if target_text.is_empty() {
            return Err(CoreError::Execution {
                component: self.name().to_string(),
                message: "field `target` must not be empty".to_string(),
            });
        }

        let target_path = resolve_workspace_path(&self.workspace_root, path_str)?;
        let content = fs::read_to_string(&target_path).await?;
        let replacements = content.matches(target_text).count();
        if replacements == 0 {
            return Err(CoreError::Execution {
                component: self.name().to_string(),
                message: format!("target text not found in `{path_str}`"),
            });
        }

        let updated = content.replace(target_text, replacement);
        fs::write(&target_path, updated.as_bytes()).await?;

        Ok(json!({
            "path": path_str,
            "replacements": replacements,
        }))
    }
}

/// Built-in [`AgentTool`] for executing shell commands inside `workspace_root`.
pub struct RunCommandTool {
    workspace_root: PathBuf,
}

impl RunCommandTool {
    /// Creates a [`RunCommandTool`] rooted at `workspace_root`.
    #[must_use]
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    async fn run_with_timeout(&self, command: &str, timeout: Duration) -> CoreResult<Value> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .kill_on_drop(true);

        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| CoreError::Execution {
                component: self.name().to_string(),
                message: format!("command timed out after {}s", timeout.as_secs()),
            })??;

        Ok(json!({
            "exit_code": output.status.code().unwrap_or(-1),
            "stdout": String::from_utf8_lossy(&output.stdout).into_owned(),
            "stderr": String::from_utf8_lossy(&output.stderr).into_owned(),
        }))
    }
}

#[async_trait]
impl AgentTool for RunCommandTool {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command inside the workspace root via `sh -c` (30s timeout)"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to execute" }
            },
            "required": ["command"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> CoreResult<Value> {
        let command = required_str(&input, "command", self.name())?;
        if command.trim().is_empty() {
            return Err(CoreError::Execution {
                component: self.name().to_string(),
                message: "field `command` must not be empty".to_string(),
            });
        }
        self.run_with_timeout(command, COMMAND_TIMEOUT).await
    }
}

/// Registers built-in workspace inspection and coding tools into [`ExtensionRegistry`].
pub fn register_builtin_tools(registry: &mut ExtensionRegistry, workspace_root: &Path) {
    registry.register_tool(Arc::new(ReadFileTool::new(workspace_root)));
    registry.register_tool(Arc::new(ListDirTool::new(workspace_root)));
    registry.register_tool(Arc::new(GrepSearchTool::new(workspace_root)));
    registry.register_tool(Arc::new(WriteFileTool::new(workspace_root)));
    registry.register_tool(Arc::new(EditFileTool::new(workspace_root)));
    registry.register_tool(Arc::new(RunCommandTool::new(workspace_root)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruvi_core::SessionId;

    struct TempWorkspace(PathBuf);

    impl TempWorkspace {
        fn new(prefix: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("ruvi-{prefix}-{}", SessionId::new()));
            std::fs::create_dir_all(&dir).expect("temp workspace should be created");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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

    #[tokio::test]
    async fn grep_search_finds_utf8_matches_skips_ignored_and_caps_at_50() {
        let ws = TempWorkspace::new("grep");
        std::fs::create_dir_all(ws.path().join("src")).expect("src dir");
        std::fs::create_dir_all(ws.path().join(".git")).expect(".git dir");
        std::fs::create_dir_all(ws.path().join("target")).expect("target dir");

        std::fs::write(ws.path().join(".git/config"), "needle in git\n").expect("git file");
        std::fs::write(ws.path().join("target/build.log"), "needle in target\n").expect("target");
        std::fs::write(
            ws.path().join("binary.bin"),
            [0xff, 0xfe, b'n', b'e', b'e', b'd', b'l', b'e'],
        )
        .expect("binary file");
        std::fs::write(
            ws.path().join("src/a.rs"),
            "first line\nlet marker = \"needle\";\n",
        )
        .expect("a.rs");

        let mut many = String::new();
        for i in 1..=60 {
            let _ = writeln!(many, "line {i}: needle");
        }
        std::fs::write(ws.path().join("src/z_many.rs"), many).expect("z_many.rs");

        let grep = GrepSearchTool::new(ws.path());
        assert!(grep.is_read_only());

        let missing_err = grep
            .execute(json!({}))
            .await
            .expect_err("missing query should fail");
        assert!(matches!(missing_err, CoreError::Execution { .. }));

        let empty_err = grep
            .execute(json!({ "query": "" }))
            .await
            .expect_err("empty query should fail");
        assert!(matches!(empty_err, CoreError::Execution { .. }));

        let res = grep
            .execute(json!({ "query": "needle" }))
            .await
            .expect("grep_search should succeed");
        let matches = res["matches"].as_array().expect("matches array");
        assert_eq!(matches.len(), 50);
        assert_eq!(res["truncated"], true);
        assert_eq!(matches[0]["path"], "src/a.rs");
        assert_eq!(matches[0]["line"], 2);
        assert_eq!(matches[0]["text"], "let marker = \"needle\";");
        assert!(!matches.iter().any(|m| {
            m["path"]
                .as_str()
                .is_some_and(|p| p.contains(".git") || p.contains("target"))
        }));
    }

    #[tokio::test]
    async fn write_file_creates_nested_files_and_rejects_traversal() {
        let ws = TempWorkspace::new("write");
        let tool = WriteFileTool::new(ws.path());
        assert!(!tool.is_read_only());

        let res = tool
            .execute(json!({
                "path": "nested/dir/hello.txt",
                "content": "hello ruvi"
            }))
            .await
            .expect("write_file should succeed");
        assert_eq!(res["path"], "nested/dir/hello.txt");
        assert_eq!(res["bytes_written"], 10);
        assert_eq!(
            std::fs::read_to_string(ws.path().join("nested/dir/hello.txt"))
                .expect("written file should be readable"),
            "hello ruvi"
        );

        // Overwrite existing file cleanly without trailing-slash issues.
        let overwrite = tool
            .execute(json!({
                "path": "nested/dir/hello.txt",
                "content": "updated"
            }))
            .await
            .expect("overwrite should succeed");
        assert_eq!(overwrite["bytes_written"], 7);

        for bad in [
            "../escape.txt",
            "nested/../../escape.txt",
            "/etc/passwd",
            "",
        ] {
            let err = tool
                .execute(json!({ "path": bad, "content": "nope" }))
                .await
                .expect_err("escaping or empty path must be rejected");
            assert!(matches!(err, CoreError::Execution { .. }));
        }

        #[cfg(unix)]
        {
            let outside = TempWorkspace::new("outside");
            std::os::unix::fs::symlink(outside.path(), ws.path().join("out_link"))
                .expect("symlink creation");
            let err = tool
                .execute(json!({
                    "path": "out_link/subdir/escape.txt",
                    "content": "nope"
                }))
                .await
                .expect_err("symlink escape must be rejected");
            assert!(matches!(err, CoreError::Execution { .. }));
            assert!(!outside.path().join("subdir/escape.txt").exists());
        }
    }

    #[tokio::test]
    async fn edit_file_replaces_target_and_rejects_empty_or_missing_target() {
        let ws = TempWorkspace::new("edit");
        std::fs::write(ws.path().join("lib.rs"), "let x = 1;\nlet y = 2;\n").expect("seed file");

        let tool = EditFileTool::new(ws.path());
        assert!(!tool.is_read_only());

        let res = tool
            .execute(json!({
                "path": "lib.rs",
                "target": "let x = 1;",
                "replacement": "let x = 42;"
            }))
            .await
            .expect("edit_file should succeed");
        assert_eq!(res["path"], "lib.rs");
        assert_eq!(res["replacements"], 1);
        assert_eq!(
            std::fs::read_to_string(ws.path().join("lib.rs")).expect("read edited file"),
            "let x = 42;\nlet y = 2;\n"
        );

        let empty_err = tool
            .execute(json!({
                "path": "lib.rs",
                "target": "",
                "replacement": "abc"
            }))
            .await
            .expect_err("empty target must fail");
        assert!(matches!(empty_err, CoreError::Execution { .. }));

        let missing_err = tool
            .execute(json!({
                "path": "lib.rs",
                "target": "nonexistent",
                "replacement": "abc"
            }))
            .await
            .expect_err("missing target must fail");
        assert!(matches!(missing_err, CoreError::Execution { .. }));
    }

    #[tokio::test]
    async fn run_command_executes_in_workspace_and_respects_timeout() {
        let ws = TempWorkspace::new("cmd");
        std::fs::write(ws.path().join("note.txt"), "from workspace\n").expect("seed note");

        let tool = RunCommandTool::new(ws.path());
        assert!(!tool.is_read_only());

        let ok_res = tool
            .execute(json!({ "command": "cat note.txt && printf warn >&2" }))
            .await
            .expect("run_command should succeed");
        assert_eq!(ok_res["exit_code"], 0);
        assert_eq!(ok_res["stdout"], "from workspace\n");
        assert_eq!(ok_res["stderr"], "warn");

        let nonzero = tool
            .execute(json!({ "command": "exit 7" }))
            .await
            .expect("non-zero exit should still return JSON output");
        assert_eq!(nonzero["exit_code"], 7);

        let timeout_err = tool
            .run_with_timeout("sleep 5", Duration::from_millis(50))
            .await
            .expect_err("timed-out command must error");
        assert!(matches!(timeout_err, CoreError::Execution { .. }));
    }

    #[test]
    fn register_builtin_tools_registers_all_tools_with_expected_permissions() {
        let ws = TempWorkspace::new("reg");
        let mut registry = ExtensionRegistry::new();
        register_builtin_tools(&mut registry, ws.path());

        assert!(
            registry
                .tool("read_file")
                .expect("read_file")
                .is_read_only()
        );
        assert!(registry.tool("list_dir").expect("list_dir").is_read_only());
        assert!(
            registry
                .tool("grep_search")
                .expect("grep_search")
                .is_read_only()
        );
        assert!(
            !registry
                .tool("write_file")
                .expect("write_file")
                .is_read_only()
        );
        assert!(
            !registry
                .tool("edit_file")
                .expect("edit_file")
                .is_read_only()
        );
        assert!(
            !registry
                .tool("run_command")
                .expect("run_command")
                .is_read_only()
        );
    }
}
