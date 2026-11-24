use std::path::Path;

use async_trait::async_trait;
use ruvi_core::{
    AgentMode, AgentProfile, ContextLoader, CoreResult, HarnessContext, McpServerConfig,
    RuleDocument, SkillDescriptor,
};
use serde_json::Value;
use tokio::fs;

/// Discovers rules, skills, agent profiles, and MCP servers compatible with `RuviCode`, `OpenCode`, Claude Code, and Cursor.
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

    async fn load_agents_dir(
        workspace: &Path,
        relative_dir: &str,
        source_harness: &str,
        agents: &mut Vec<AgentProfile>,
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
                let fallback_name = path.file_stem().map_or_else(
                    || "unknown".to_string(),
                    |n| n.to_string_lossy().to_string(),
                );
                agents.push(parse_agent_markdown(
                    &content,
                    &fallback_name,
                    source_harness,
                ));
            }
        }
        Ok(())
    }

    async fn load_opencode_agents_json(
        workspace: &Path,
        agents: &mut Vec<AgentProfile>,
    ) -> CoreResult<()> {
        let path = workspace.join("opencode.json");
        if !path.is_file() {
            return Ok(());
        }
        let raw = fs::read_to_string(&path).await?;
        let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&raw) else {
            return Ok(());
        };
        let Some(Value::Object(map)) = root.get("agent") else {
            return Ok(());
        };

        for (name, cfg) in map {
            let Some(obj) = cfg.as_object() else {
                continue;
            };
            let description = obj
                .get("description")
                .and_then(Value::as_str)
                .map_or_else(String::new, |s| s.trim().to_string());
            let system_prompt = obj
                .get("prompt")
                .and_then(Value::as_str)
                .map_or_else(String::new, |s| s.trim().to_string());
            let model = obj
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string);
            let mode = obj
                .get("mode")
                .and_then(Value::as_str)
                .and_then(parse_agent_mode);

            agents.push(AgentProfile {
                name: name.clone(),
                description,
                system_prompt,
                model,
                mode,
                source_harness: "opencode".to_string(),
            });
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

        Self::load_agents_dir(workspace, ".agents/agents", "agents", &mut ctx.agents).await?;
        Self::load_agents_dir(workspace, ".claude/agents", "claude", &mut ctx.agents).await?;
        Self::load_agents_dir(workspace, ".opencode/agent", "opencode", &mut ctx.agents).await?;
        Self::load_opencode_agents_json(workspace, &mut ctx.agents).await?;

        ctx.rules.sort_by(|a, b| a.name.cmp(&b.name));
        ctx.skills.sort_by(|a, b| a.name.cmp(&b.name));
        ctx.mcp_servers.sort_by(|a, b| a.name.cmp(&b.name));
        ctx.agents.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(ctx)
    }
}

#[derive(Default)]
struct Frontmatter {
    name: Option<String>,
    description: String,
    model: Option<String>,
    mode: Option<AgentMode>,
}

/// Parses optional YAML frontmatter (`--- ... ---`) and returns parsed fields plus trimmed markdown body.
fn parse_frontmatter(content: &str) -> (Frontmatter, String) {
    let mut lines = content.split_inclusive('\n');
    if lines.next().map(str::trim) != Some("---") {
        return (Frontmatter::default(), content.trim().to_string());
    }

    let mut fm = Frontmatter::default();
    let mut in_multiline_desc = false;
    let mut closed = false;

    for line in &mut lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            closed = true;
            break;
        }

        if in_multiline_desc {
            if line.starts_with(' ') || line.starts_with('\t') {
                if !fm.description.is_empty() {
                    fm.description.push(' ');
                }
                fm.description.push_str(trimmed);
                continue;
            }
            in_multiline_desc = false;
        }

        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }

        if let Some(raw_name) = trimmed.strip_prefix("name:") {
            let parsed = unquote(raw_name.trim());
            if !parsed.is_empty() {
                fm.name = Some(parsed);
            }
        } else if let Some(raw_desc) = trimmed.strip_prefix("description:") {
            let value = raw_desc.trim();
            if matches!(value, ">" | "|-" | "|" | ">-") {
                in_multiline_desc = true;
            } else {
                fm.description = unquote(value);
            }
        } else if let Some(raw_model) = trimmed.strip_prefix("model:") {
            let parsed = unquote(raw_model.trim());
            if !parsed.is_empty() {
                fm.model = Some(parsed);
            }
        } else if let Some(raw_mode) = trimmed.strip_prefix("mode:") {
            fm.mode = parse_agent_mode(&unquote(raw_mode.trim()));
        }
    }

    let body = if closed {
        lines.collect::<String>().trim().to_string()
    } else {
        String::new()
    };
    (fm, body)
}

/// Extracts `name` and `description` from YAML frontmatter (`--- ... ---`), supporting folded `>`/`|` scalars.
fn parse_skill_frontmatter(content: &str, fallback_name: &str) -> (String, String) {
    let (fm, _) = parse_frontmatter(content);
    (
        fm.name.unwrap_or_else(|| fallback_name.to_string()),
        fm.description,
    )
}

fn parse_agent_markdown(content: &str, fallback_name: &str, source_harness: &str) -> AgentProfile {
    let (fm, system_prompt) = parse_frontmatter(content);
    AgentProfile {
        name: fm.name.unwrap_or_else(|| fallback_name.to_string()),
        description: fm.description,
        system_prompt,
        model: fm.model,
        mode: fm.mode,
        source_harness: source_harness.to_string(),
    }
}

fn parse_agent_mode(raw: &str) -> Option<AgentMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "build" => Some(AgentMode::Build),
        "plan" => Some(AgentMode::Plan),
        _ => None,
    }
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

    #[tokio::test]
    async fn discovers_agent_profiles_from_markdown_and_opencode_json() {
        let temp_dir = std::env::temp_dir().join(format!(
            "ruvi-harness-agents-{}",
            ruvi_core::SessionId::new()
        ));
        let agents_dir = temp_dir.join(".agents/agents");
        let claude_dir = temp_dir.join(".claude/agents");
        let opencode_dir = temp_dir.join(".opencode/agent");
        std::fs::create_dir_all(&agents_dir).expect("create .agents/agents");
        std::fs::create_dir_all(&claude_dir).expect("create .claude/agents");
        std::fs::create_dir_all(&opencode_dir).expect("create .opencode/agent");

        std::fs::write(
            agents_dir.join("reviewer.md"),
            "---\nname: reviewer\ndescription: >\n  Strict code review\n  specialist\nmodel: openai/gpt-4o\nmode: plan\n---\n\nInspect diffs carefully for bugs and edge cases.\n",
        )
        .expect("write reviewer.md");
        std::fs::write(
            claude_dir.join("debugger.md"),
            "---\ndescription: Root-cause debugger\nmode: build\n---\nReproduce failures before fixing.\n",
        )
        .expect("write debugger.md");
        std::fs::write(
            opencode_dir.join("planner.md"),
            "---\nname: planner\ndescription: Task planner\nmode: plan\n---\nBreak work into small steps.\n",
        )
        .expect("write planner.md");
        std::fs::write(
            temp_dir.join("opencode.json"),
            r#"{
  "agent": {
    "architect": {
      "description": "System architect",
      "model": "anthropic/claude-sonnet-4",
      "mode": "plan",
      "prompt": "Design clean vertical slices."
    }
  }
}"#,
        )
        .expect("write opencode.json");

        let ctx = MultiHarnessContextLoader
            .load(&temp_dir)
            .await
            .expect("context loader should succeed");

        let _ = std::fs::remove_dir_all(&temp_dir);

        let names: Vec<&str> = ctx.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["architect", "debugger", "planner", "reviewer"]);

        let reviewer = ctx
            .agents
            .iter()
            .find(|a| a.name == "reviewer")
            .expect("reviewer profile");
        assert_eq!(reviewer.description, "Strict code review specialist");
        assert_eq!(reviewer.model.as_deref(), Some("openai/gpt-4o"));
        assert_eq!(reviewer.mode, Some(AgentMode::Plan));
        assert_eq!(
            reviewer.system_prompt,
            "Inspect diffs carefully for bugs and edge cases."
        );
        assert_eq!(reviewer.source_harness, "agents");

        let architect = ctx
            .agents
            .iter()
            .find(|a| a.name == "architect")
            .expect("architect profile");
        assert_eq!(architect.description, "System architect");
        assert_eq!(
            architect.model.as_deref(),
            Some("anthropic/claude-sonnet-4")
        );
        assert_eq!(architect.mode, Some(AgentMode::Plan));
        assert_eq!(architect.system_prompt, "Design clean vertical slices.");
        assert_eq!(architect.source_harness, "opencode");

        let debugger = ctx
            .agents
            .iter()
            .find(|a| a.name == "debugger")
            .expect("debugger profile");
        assert_eq!(debugger.mode, Some(AgentMode::Build));
        assert_eq!(debugger.source_harness, "claude");
    }
}
