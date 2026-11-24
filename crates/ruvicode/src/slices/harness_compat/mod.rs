use std::path::Path;

use async_trait::async_trait;
use ruvi_core::{
    ContextLoader, CoreResult, HarnessContext, McpServerConfig, RuleDocument, SkillDescriptor,
};
use serde_json::Value;
use tokio::fs;

/// Discovers rules, skills, and MCP servers compatible with `RuviCode`, `OpenCode`, Claude Code, and Cursor.
pub struct MultiHarnessContextLoader;

impl MultiHarnessContextLoader {
    async fn load_rule_if_exists(
        workspace: &Path,
        relative: &str,
        rules: &mut Vec<RuleDocument>,
    ) -> CoreResult<()> {
        let path = workspace.join(relative);
        if path.is_file() {
            let content = fs::read_to_string(&path).await?;
            rules.push(RuleDocument {
                name: relative.to_string(),
                content,
                path,
            });
        }
        Ok(())
    }

    async fn load_rules_dir(
        workspace: &Path,
        relative_dir: &str,
        rules: &mut Vec<RuleDocument>,
    ) -> CoreResult<()> {
        let dir = workspace.join(relative_dir);
        if !dir.is_dir() {
            return Ok(());
        }
        let mut entries = fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                let content = fs::read_to_string(&path).await?;
                let file_name = path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().to_string());
                rules.push(RuleDocument {
                    name: format!("{relative_dir}/{file_name}"),
                    content,
                    path,
                });
            }
        }
        Ok(())
    }

    async fn load_skills_dir(
        workspace: &Path,
        relative_dir: &str,
        source_harness: &str,
        skills: &mut Vec<SkillDescriptor>,
    ) -> CoreResult<()> {
        let dir = workspace.join(relative_dir);
        if !dir.is_dir() {
            return Ok(());
        }
        let mut entries = fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }
            let skill_file = skill_dir.join("SKILL.md");
            if !skill_file.is_file() {
                continue;
            }
            let content = fs::read_to_string(&skill_file).await?;
            let fallback_name = skill_dir.file_name().map_or_else(
                || "unknown".to_string(),
                |n| n.to_string_lossy().to_string(),
            );
            let (name, description) = parse_skill_frontmatter(&content, &fallback_name);
            skills.push(SkillDescriptor {
                name,
                description,
                path: skill_file,
                source_harness: source_harness.to_string(),
            });
        }
        Ok(())
    }

    async fn load_mcp_json(
        workspace: &Path,
        relative: &str,
        root_key: &str,
        servers: &mut Vec<McpServerConfig>,
    ) -> CoreResult<()> {
        let path = workspace.join(relative);
        if !path.is_file() {
            return Ok(());
        }
        let raw = fs::read_to_string(&path).await?;
        let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&raw) else {
            return Ok(());
        };
        let Some(Value::Object(map)) = root.get(root_key) else {
            return Ok(());
        };

        for (name, cfg) in map {
            let target = match cfg.get("command") {
                Some(Value::String(cmd)) => cmd.clone(),
                Some(Value::Array(arr)) => arr
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                _ => cfg
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            };

            let mut args = Vec::new();
            if let Some(Value::Array(arr)) = cfg.get("args") {
                args.extend(
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string),
                );
            } else if let Some(Value::Array(cmd_arr)) = cfg.get("command") {
                args.extend(
                    cmd_arr
                        .iter()
                        .skip(1)
                        .filter_map(Value::as_str)
                        .map(ToString::to_string),
                );
            }

            if !target.is_empty() {
                servers.push(McpServerConfig {
                    name: name.clone(),
                    target,
                    args,
                    source_file: relative.to_string(),
                });
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ContextLoader for MultiHarnessContextLoader {
    fn id(&self) -> &'static str {
        "multi-harness"
    }

    async fn load(&self, workspace: &Path) -> CoreResult<HarnessContext> {
        let mut ctx = HarnessContext::default();

        for file in ["agent.md", "AGENTS.md", "CLAUDE.md"] {
            Self::load_rule_if_exists(workspace, file, &mut ctx.rules).await?;
        }
        Self::load_rules_dir(workspace, ".agents/rules", &mut ctx.rules).await?;
        Self::load_rules_dir(workspace, ".cursor/rules", &mut ctx.rules).await?;

        Self::load_skills_dir(workspace, ".agents/skills", "agents", &mut ctx.skills).await?;
        Self::load_skills_dir(workspace, ".claude/skills", "claude", &mut ctx.skills).await?;
        Self::load_skills_dir(workspace, ".opencode/skills", "opencode", &mut ctx.skills).await?;

        Self::load_mcp_json(workspace, ".mcp.json", "mcpServers", &mut ctx.mcp_servers).await?;
        Self::load_mcp_json(workspace, "opencode.json", "mcp", &mut ctx.mcp_servers).await?;

        ctx.rules.sort_by(|a, b| a.name.cmp(&b.name));
        ctx.skills.sort_by(|a, b| a.name.cmp(&b.name));
        ctx.mcp_servers.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(ctx)
    }
}

/// Extracts `name` and `description` from YAML frontmatter (`--- ... ---`), supporting folded `>`/`|` scalars.
fn parse_skill_frontmatter(content: &str, fallback_name: &str) -> (String, String) {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (fallback_name.to_string(), String::new());
    }

    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut in_multiline_desc = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }

        if in_multiline_desc {
            if line.starts_with(' ') || line.starts_with('\t') {
                if !description.is_empty() {
                    description.push(' ');
                }
                description.push_str(trimmed);
                continue;
            }
            in_multiline_desc = false;
        }

        if let Some(raw_name) = trimmed.strip_prefix("name:") {
            let parsed = unquote(raw_name.trim());
            if !parsed.is_empty() {
                name = parsed;
            }
        } else if let Some(raw_desc) = trimmed.strip_prefix("description:") {
            let value = raw_desc.trim();
            if matches!(value, ">" | "|-" | "|" | ">-") {
                in_multiline_desc = true;
            } else {
                description = unquote(value);
            }
        }
    }

    (name, description)
}

fn unquote(raw: &str) -> String {
    raw.trim_matches('"').trim_matches('\'').trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discovers_workspace_agent_md_and_installed_skills() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");

        let ctx = MultiHarnessContextLoader
            .load(&repo_root)
            .await
            .expect("context loader should succeed");

        assert!(ctx.rules.iter().any(|r| r.name == "agent.md"));
        assert!(ctx.skills.iter().any(|s| s.name == "ponytail"));
        assert!(ctx.skills.iter().any(|s| s.name == "rust-anything"));
    }
}
