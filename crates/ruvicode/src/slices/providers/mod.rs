use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use ruvi_core::{
    CompletionRequest, CompletionResponse, CoreError, CoreResult, ExtensionRegistry, LlmProvider,
    MessageRole,
};
use serde::{Deserialize, Serialize};

/// Generic `OpenAI`-compatible HTTP provider supporting `Ollama`, `llama.cpp`, `OpenAI`, `OpenRouter`, and custom endpoints.
pub struct OpenAiCompatibleProvider {
    id: String,
    display_name: String,
    base_url: String,
    api_key_env: Option<String>,
    default_models: Vec<String>,
    client: Client,
}

impl OpenAiCompatibleProvider {
    /// Creates a new [`OpenAiCompatibleProvider`].
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        base_url: impl Into<String>,
        api_key_env: Option<&str>,
        default_models: Vec<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            id: id.into(),
            display_name: display_name.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key_env: api_key_env.map(ToString::to_string),
            default_models,
            client,
        }
    }

    fn resolve_api_key(&self) -> CoreResult<Option<String>> {
        let Some(env_name) = &self.api_key_env else {
            return Ok(None);
        };
        let key = std::env::var(env_name).unwrap_or_default();
        if key.trim().is_empty() {
            return Err(CoreError::Config(format!(
                "missing API key environment variable `{env_name}` for provider `{}`",
                self.id
            )));
        }
        Ok(Some(key))
    }

    async fn fetch_ollama_tags(&self) -> Option<Vec<String>> {
        let root = self.base_url.strip_suffix("/v1").unwrap_or(&self.base_url);
        let url = format!("{root}/api/tags");
        let resp = self
            .client
            .get(url)
            .timeout(Duration::from_millis(700))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: OllamaTagsResponse = resp.json().await.ok()?;
        let models: Vec<String> = body.models.into_iter().map(|m| m.name).collect();
        if models.is_empty() {
            None
        } else {
            Some(models)
        }
    }

    async fn fetch_openai_models(&self, api_key: Option<&str>) -> Option<Vec<String>> {
        let url = format!("{}/models", self.base_url);
        let mut req = self.client.get(url).timeout(Duration::from_millis(900));
        if let Some(token) = api_key {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let body: OpenAiModelsResponse = resp.json().await.ok()?;
        let models: Vec<String> = body.data.into_iter().take(12).map(|m| m.id).collect();
        if models.is_empty() {
            None
        } else {
            Some(models)
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    async fn list_models(&self) -> CoreResult<Vec<String>> {
        if self.id == "ollama"
            && let Some(models) = self.fetch_ollama_tags().await
        {
            return Ok(models);
        }

        let api_key = self
            .api_key_env
            .as_ref()
            .and_then(|k| std::env::var(k).ok());
        if (self.api_key_env.is_none() || api_key.as_deref().is_some_and(|v| !v.is_empty()))
            && let Some(models) = self.fetch_openai_models(api_key.as_deref()).await
        {
            return Ok(models);
        }

        Ok(self.default_models.clone())
    }

    async fn complete(&self, request: CompletionRequest) -> CoreResult<CompletionResponse> {
        let api_key = self.resolve_api_key()?;
        let url = format!("{}/chat/completions", self.base_url);

        let payload = ChatCompletionPayload {
            model: request.model.clone(),
            messages: request
                .messages
                .into_iter()
                .map(|m| ChatCompletionMsg {
                    role: match m.role {
                        MessageRole::System => "system",
                        MessageRole::User => "user",
                        MessageRole::Assistant => "assistant",
                        MessageRole::Tool => "tool",
                    },
                    content: m.content,
                })
                .collect(),
        };

        let mut req = self.client.post(url).json(&payload);
        if let Some(token) = api_key {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.map_err(|err| CoreError::Provider {
            provider: self.id.clone(),
            message: err.to_string(),
        })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(CoreError::Provider {
                provider: self.id.clone(),
                message: format!("HTTP {status}: {body}"),
            });
        }

        let parsed: ChatCompletionReply =
            response.json().await.map_err(|err| CoreError::Provider {
                provider: self.id.clone(),
                message: format!("invalid response JSON: {err}"),
            })?;

        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_else(|| "(empty completion)".to_string());

        Ok(CompletionResponse {
            model: parsed.model.unwrap_or(request.model),
            content,
        })
    }
}

/// Zero-config offline fallback provider for instant evaluation and diagnostics.
pub struct LocalEchoProvider;

#[async_trait]
impl LlmProvider for LocalEchoProvider {
    fn id(&self) -> &'static str {
        "ruvi-demo"
    }

    fn display_name(&self) -> &'static str {
        "RuviCode Offline Demo"
    }

    async fn list_models(&self) -> CoreResult<Vec<String>> {
        Ok(vec!["ruvi-demo-v1".to_string()])
    }

    async fn complete(&self, request: CompletionRequest) -> CoreResult<CompletionResponse> {
        let user_prompt = request
            .messages
            .iter()
            .rfind(|m| m.role == MessageRole::User)
            .map_or("", |m| m.content.as_str());

        let skills_summary = request
            .messages
            .iter()
            .find(|m| m.role == MessageRole::System)
            .map_or(0, |m| {
                m.content
                    .lines()
                    .filter(|l| l.starts_with("- ") && l.contains("): "))
                    .count()
            });

        let reply = format!(
            "[{}] RuviCode Offline Demo received your prompt ({skills_summary} skills loaded in context).\n\
             Prompt: {user_prompt}\n\n\
             Tip: Use `/local` (Ctrl+L) to install/pull Ollama or llama.cpp models, `/models` to switch provider, \
             or export OPENAI_API_KEY / OPENROUTER_API_KEY.",
            request.mode.badge()
        );

        Ok(CompletionResponse {
            model: request.model,
            content: reply,
        })
    }
}

/// Registers built-in `OpenAI`-compatible providers and offline demo fallback into [`ExtensionRegistry`].
pub fn register_default_providers(registry: &mut ExtensionRegistry) {
    let ollama_url = std::env::var("OLLAMA_HOST")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map_or_else(
            || "http://127.0.0.1:11434/v1".to_string(),
            |host| format!("{}/v1", host.trim_end_matches('/')),
        );
    let llama_url = std::env::var("LLAMA_CPP_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8080/v1".to_string());
    let anthropic_url = std::env::var("ANTHROPIC_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string());

    registry.register_provider(Arc::new(OpenAiCompatibleProvider::new(
        "ollama",
        "Ollama (Local)",
        ollama_url,
        None,
        vec![
            "qwen2.5-coder:7b".to_string(),
            "llama3.2:latest".to_string(),
        ],
    )));
    registry.register_provider(Arc::new(OpenAiCompatibleProvider::new(
        "llama-cpp",
        "llama.cpp Server (Local)",
        llama_url,
        None,
        vec!["local-gguf".to_string()],
    )));
    registry.register_provider(Arc::new(OpenAiCompatibleProvider::new(
        "openai",
        "OpenAI",
        "https://api.openai.com/v1",
        Some("OPENAI_API_KEY"),
        vec!["gpt-4o-mini".to_string(), "gpt-4.1".to_string()],
    )));
    registry.register_provider(Arc::new(OpenAiCompatibleProvider::new(
        "openrouter",
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        Some("OPENROUTER_API_KEY"),
        vec![
            "anthropic/claude-3.7-sonnet".to_string(),
            "openai/gpt-4o-mini".to_string(),
        ],
    )));
    registry.register_provider(Arc::new(OpenAiCompatibleProvider::new(
        "anthropic-compat",
        "Anthropic-Compatible",
        anthropic_url,
        Some("ANTHROPIC_API_KEY"),
        vec!["claude-3-7-sonnet-latest".to_string()],
    )));
    registry.register_provider(Arc::new(LocalEchoProvider));
}

/// Selects the best ready provider & model without blocking startup.
pub async fn detect_initial_provider_and_model(registry: &ExtensionRegistry) -> (String, String) {
    if let Ok(ollama) = registry.provider("ollama")
        && let Ok(resp) = Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .unwrap_or_else(|_| Client::new())
            .get("http://127.0.0.1:11434/api/tags")
            .send()
            .await
        && resp.status().is_success()
        && let Ok(tags) = resp.json::<OllamaTagsResponse>().await
        && let Some(first) = tags.models.into_iter().next()
    {
        let _ = ollama;
        return ("ollama".to_string(), first.name);
    }

    if std::env::var("OPENAI_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
        return ("openai".to_string(), "gpt-4o-mini".to_string());
    }

    if std::env::var("OPENROUTER_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
        return (
            "openrouter".to_string(),
            "anthropic/claude-3.7-sonnet".to_string(),
        );
    }

    ("ruvi-demo".to_string(), "ruvi-demo-v1".to_string())
}

#[derive(Serialize)]
struct ChatCompletionPayload {
    model: String,
    messages: Vec<ChatCompletionMsg>,
}

#[derive(Serialize)]
struct ChatCompletionMsg {
    role: &'static str,
    content: String,
}

#[derive(Deserialize)]
struct ChatCompletionReply {
    model: Option<String>,
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionReplyMsg,
}

#[derive(Deserialize)]
struct ChatCompletionReplyMsg {
    content: Option<String>,
}

#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaTagModel>,
}

#[derive(Deserialize)]
struct OllamaTagModel {
    name: String,
}

#[derive(Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelEntry>,
}

#[derive(Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

#[cfg(test)]
mod tests {
    use ruvi_core::{AgentMode, ChatMessage};

    use super::*;

    #[tokio::test]
    async fn demo_provider_completes_with_mode_badge() {
        let provider = LocalEchoProvider;
        let reply = provider
            .complete(CompletionRequest {
                model: "ruvi-demo-v1".to_string(),
                messages: vec![ChatMessage::new(MessageRole::User, "ping")],
                mode: AgentMode::Plan,
            })
            .await
            .expect("demo provider must succeed");

        assert!(reply.content.contains("[PLAN]"));
        assert!(reply.content.contains("ping"));
    }

    #[tokio::test]
    async fn openai_provider_errors_cleanly_when_api_key_missing() {
        let provider = OpenAiCompatibleProvider::new(
            "missing-key-test",
            "Missing Key",
            "https://example.invalid/v1",
            Some("RUVI_NONEXISTENT_API_KEY_TEST"),
            vec!["test-model".to_string()],
        );
        let err = provider
            .complete(CompletionRequest {
                model: "test-model".to_string(),
                messages: vec![],
                mode: AgentMode::Build,
            })
            .await
            .expect_err("missing env key must return CoreError::Config");

        assert!(matches!(err, CoreError::Config(_)));
    }
}
