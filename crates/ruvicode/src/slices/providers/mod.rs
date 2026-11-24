use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use ruvi_core::{
    CompletionRequest, CompletionResponse, CoreError, CoreResult, ExtensionRegistry, LlmProvider,
    MessageRole, ToolCall,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

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

    fn build_payload(request: &CompletionRequest, stream: bool) -> ChatCompletionPayload {
        let messages = request
            .messages
            .iter()
            .map(|m| ChatCompletionMsg {
                role: match m.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                },
                content: m.content.clone(),
                tool_calls: m
                    .tool_calls
                    .iter()
                    .map(|tc| WireToolCall {
                        id: tc.id.clone(),
                        kind: "function".to_string(),
                        function: WireToolFunction {
                            name: tc.name.clone(),
                            arguments: tc.arguments.to_string(),
                        },
                    })
                    .collect(),
                tool_call_id: m.tool_call_id.clone(),
            })
            .collect();

        let tools = request
            .tools
            .iter()
            .map(|t| WireToolDef {
                kind: "function",
                function: WireToolDefFn {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect();

        ChatCompletionPayload {
            model: request.model.clone(),
            messages,
            tools,
            stream,
        }
    }

    async fn send_chat_request(
        &self,
        payload: &ChatCompletionPayload,
    ) -> CoreResult<reqwest::Response> {
        let api_key = self.resolve_api_key()?;
        let url = format!("{}/chat/completions", self.base_url);
        let mut req = self.client.post(url).json(payload);
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
        Ok(response)
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
        let payload = Self::build_payload(&request, false);
        let response = self.send_chat_request(&payload).await?;

        let parsed: ChatCompletionReply =
            response.json().await.map_err(|err| CoreError::Provider {
                provider: self.id.clone(),
                message: format!("invalid response JSON: {err}"),
            })?;

        let msg = parsed.choices.into_iter().next().map(|c| c.message);
        let tool_calls = msg
            .as_ref()
            .map(|m| parse_wire_tool_calls(&m.tool_calls))
            .unwrap_or_default();
        let content = msg.and_then(|m| m.content).unwrap_or_else(|| {
            if tool_calls.is_empty() {
                "(empty completion)".to_string()
            } else {
                String::new()
            }
        });

        Ok(CompletionResponse {
            model: parsed.model.unwrap_or(request.model),
            content,
            tool_calls,
        })
    }

    async fn complete_stream(
        &self,
        request: CompletionRequest,
        token_tx: mpsc::UnboundedSender<String>,
    ) -> CoreResult<CompletionResponse> {
        let payload = Self::build_payload(&request, true);
        let mut response = self.send_chat_request(&payload).await?;

        let mut acc = SseStreamAccumulator::new(request.model.clone());
        let mut line_buf = String::new();

        while let Some(chunk) = response.chunk().await.map_err(|err| CoreError::Provider {
            provider: self.id.clone(),
            message: format!("stream read error: {err}"),
        })? {
            let piece = String::from_utf8_lossy(&chunk);
            line_buf.push_str(&piece);
            while let Some(pos) = line_buf.find('\n') {
                let line = line_buf[..pos].trim_end_matches('\r').to_string();
                line_buf.drain(..=pos);
                acc.push_line(&line, &token_tx);
            }
        }

        if !line_buf.trim().is_empty() {
            acc.push_line(line_buf.trim_end_matches('\r'), &token_tx);
        }

        Ok(acc.finish())
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
            tool_calls: Vec::new(),
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireToolDef>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
}

#[derive(Serialize)]
struct WireToolDef {
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireToolDefFn,
}

#[derive(Serialize)]
struct WireToolDefFn {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Serialize)]
struct ChatCompletionMsg {
    role: &'static str,
    content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct WireToolCall {
    #[serde(default)]
    id: String,
    #[serde(rename = "type", default = "default_fn_type")]
    kind: String,
    #[serde(default)]
    function: WireToolFunction,
}

fn default_fn_type() -> String {
    "function".to_string()
}

#[derive(Serialize, Deserialize, Default)]
struct WireToolFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
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
    #[serde(default)]
    tool_calls: Vec<WireToolCall>,
}

#[derive(Deserialize)]
struct StreamChunkPayload {
    model: Option<String>,
    #[serde(default)]
    choices: Vec<StreamChunkChoice>,
}

#[derive(Deserialize)]
struct StreamChunkChoice {
    #[serde(default)]
    delta: StreamDelta,
}

#[derive(Deserialize, Default)]
struct StreamDelta {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<StreamToolCallDelta>,
}

#[derive(Deserialize)]
struct StreamToolCallDelta {
    #[serde(default)]
    index: usize,
    id: Option<String>,
    function: Option<StreamToolCallFnDelta>,
}

#[derive(Deserialize)]
struct StreamToolCallFnDelta {
    name: Option<String>,
    arguments: Option<String>,
}

struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

struct SseStreamAccumulator {
    model: String,
    content: String,
    tool_calls: BTreeMap<usize, PartialToolCall>,
}

impl SseStreamAccumulator {
    fn new(model: String) -> Self {
        Self {
            model,
            content: String::new(),
            tool_calls: BTreeMap::new(),
        }
    }

    fn push_line(&mut self, line: &str, token_tx: &mpsc::UnboundedSender<String>) {
        let Some(data) = line.strip_prefix("data:") else {
            return;
        };
        let payload = data.trim();
        if payload.is_empty() || payload == "[DONE]" {
            return;
        }
        let Ok(chunk) = serde_json::from_str::<StreamChunkPayload>(payload) else {
            return;
        };
        if let Some(m) = chunk.model {
            self.model = m;
        }
        for choice in chunk.choices {
            if let Some(text) = choice.delta.content
                && !text.is_empty()
            {
                self.content.push_str(&text);
                let _ = token_tx.send(text);
            }
            for tc in choice.delta.tool_calls {
                let entry = self
                    .tool_calls
                    .entry(tc.index)
                    .or_insert_with(|| PartialToolCall {
                        id: format!("call_{}", tc.index),
                        name: String::new(),
                        arguments: String::new(),
                    });
                if let Some(id) = tc.id
                    && !id.is_empty()
                {
                    entry.id = id;
                }
                if let Some(func) = tc.function {
                    if let Some(name) = func.name {
                        entry.name.push_str(&name);
                    }
                    if let Some(args) = func.arguments {
                        entry.arguments.push_str(&args);
                    }
                }
            }
        }
    }

    fn finish(self) -> CompletionResponse {
        let tool_calls = self
            .tool_calls
            .into_values()
            .filter(|tc| !tc.name.is_empty())
            .map(|tc| ToolCall {
                id: tc.id,
                name: tc.name,
                arguments: serde_json::from_str(&tc.arguments)
                    .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
            })
            .collect();

        CompletionResponse {
            model: self.model,
            content: self.content,
            tool_calls,
        }
    }
}

fn parse_wire_tool_calls(raw: &[WireToolCall]) -> Vec<ToolCall> {
    raw.iter()
        .filter(|tc| !tc.function.name.is_empty())
        .map(|tc| ToolCall {
            id: if tc.id.is_empty() {
                "call_0".to_string()
            } else {
                tc.id.clone()
            },
            name: tc.function.name.clone(),
            arguments: serde_json::from_str(&tc.function.arguments)
                .unwrap_or_else(|_| Value::Object(serde_json::Map::new())),
        })
        .collect()
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
            .complete(CompletionRequest::new(
                "ruvi-demo-v1",
                vec![ChatMessage::new(MessageRole::User, "ping")],
                AgentMode::Plan,
            ))
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
            .complete(CompletionRequest::new(
                "test-model",
                vec![],
                AgentMode::Build,
            ))
            .await
            .expect_err("missing env key must return CoreError::Config");

        assert!(matches!(err, CoreError::Config(_)));
    }

    #[test]
    fn sse_accumulator_streams_tokens_and_assembles_tool_calls() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut acc = SseStreamAccumulator::new("gpt-test".to_string());

        acc.push_line(
            r#"data: {"model":"gpt-test-1","choices":[{"delta":{"content":"Hello "}}]}"#,
            &tx,
        );
        acc.push_line(
            r#"data: {"choices":[{"delta":{"content":"Ruvi","tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_","arguments":"{\"path\":"}}]}}]}"#,
            &tx,
        );
        acc.push_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"file","arguments":"\"agent.md\"}"}}]}}]}"#,
            &tx,
        );
        acc.push_line("data: [DONE]", &tx);

        let mut streamed = String::new();
        while let Ok(tok) = rx.try_recv() {
            streamed.push_str(&tok);
        }
        assert_eq!(streamed, "Hello Ruvi");

        let resp = acc.finish();
        assert_eq!(resp.model, "gpt-test-1");
        assert_eq!(resp.content, "Hello Ruvi");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_1");
        assert_eq!(resp.tool_calls[0].name, "read_file");
        assert_eq!(resp.tool_calls[0].arguments["path"], "agent.md");
    }
}
