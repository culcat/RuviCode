use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use ruvi_core::{
    AgentMode, AppEvent, ChannelGateway, ChatMessage, CoreError, CoreResult, EventBus,
    ExtensionRegistry, MessageRole,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use crate::slices::chat::{AgentLoopConfig, execute_agent_loop};

const MAX_BOT_HISTORY_MESSAGES: usize = 40;

struct BotRouterContext {
    registry: Arc<ExtensionRegistry>,
    base_system_prompt: String,
    provider_id: String,
    model: String,
    sessions: HashMap<(String, String), Vec<ChatMessage>>,
}

/// Spawns a background task routing inbound [`AppEvent::ChannelMessageReceived`] events
/// through [`execute_agent_loop`] with per-`(channel_id, destination)` conversation history
/// and delivering replies via the matching [`ChannelGateway`].
#[must_use]
pub fn spawn_bot_session_router(
    registry: Arc<ExtensionRegistry>,
    bus: EventBus,
    base_system_prompt: String,
    provider_id: String,
    model: String,
) -> tokio::task::JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        drop(bus);
        let mut ctx = BotRouterContext {
            registry,
            base_system_prompt,
            provider_id,
            model,
            // shortcut: single-task loop processes rooms sequentially; spawn per-key tasks if multi-tenant concurrency is needed.
            sessions: HashMap::new(),
        };

        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                Err(RecvError::Lagged(skipped)) => {
                    tracing::warn!("bot session router lagged by {skipped} event(s)");
                    continue;
                }
                Err(RecvError::Closed) => break,
            };

            if let AppEvent::ChannelMessageReceived {
                channel_id,
                destination,
                sender,
                text,
            } = event
            {
                handle_inbound_bot_message(&mut ctx, channel_id, destination, sender, text).await;
            }
        }
    })
}

async fn handle_inbound_bot_message(
    ctx: &mut BotRouterContext,
    channel_id: String,
    destination: String,
    sender: String,
    text: String,
) {
    if text.trim().is_empty() {
        return;
    }

    let user_content = if sender.trim().is_empty() {
        text
    } else {
        format!("[{sender}] {text}")
    };
    let user_msg = ChatMessage::new(MessageRole::User, user_content);

    let history = ctx
        .sessions
        .entry((channel_id.clone(), destination.clone()))
        .or_default();

    let mut initial_messages = Vec::with_capacity(history.len() + 2);
    initial_messages.push(ChatMessage::new(
        MessageRole::System,
        ctx.base_system_prompt.clone(),
    ));
    initial_messages.extend(history.iter().cloned());
    initial_messages.push(user_msg.clone());

    let outcome = match execute_agent_loop(
        &ctx.registry,
        AgentLoopConfig {
            provider_id: ctx.provider_id.clone(),
            model: ctx.model.clone(),
            mode: AgentMode::Build,
            initial_messages,
        },
        None,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            tracing::warn!(
                channel_id = %channel_id,
                destination = %destination,
                "bot agent loop failed: {err}"
            );
            return;
        }
    };

    let reply_text = if outcome.response.content.trim().is_empty() {
        "(empty completion)".to_string()
    } else {
        outcome.response.content
    };

    history.push(user_msg);
    history.extend(outcome.generated_messages);
    if let Some(last) = history.last_mut()
        && last.role == MessageRole::Assistant
        && last.content.trim().is_empty()
    {
        last.content.clone_from(&reply_text);
    }
    if history.len() > MAX_BOT_HISTORY_MESSAGES {
        let excess = history.len() - MAX_BOT_HISTORY_MESSAGES;
        let cut = history[excess..]
            .iter()
            .position(|m| m.role == MessageRole::User)
            .map_or(history.len(), |offset| excess + offset);
        history.drain(..cut);
    }

    let Some(gateway) = ctx
        .registry
        .channels()
        .into_iter()
        .find(|ch| ch.id() == channel_id)
    else {
        tracing::warn!(
            channel_id = %channel_id,
            "no registered channel gateway matched inbound message"
        );
        return;
    };

    if let Err(err) = gateway.send_reply(&destination, &reply_text).await {
        tracing::warn!(
            channel_id = %channel_id,
            destination = %destination,
            "bot send_reply failed: {err}"
        );
    }
}

/// Starts every configured [`ChannelGateway`] in `registry` and spawns the session router
/// when at least one gateway starts.
///
/// Returns the IDs of the started gateways.
///
/// # Errors
/// Returns [`CoreError`] if any configured gateway fails to start.
pub async fn start_configured_bots(
    registry: Arc<ExtensionRegistry>,
    bus: EventBus,
    base_system_prompt: String,
    provider_id: String,
    model: String,
) -> CoreResult<Vec<String>> {
    let configured: Vec<_> = registry
        .channels()
        .into_iter()
        .filter(|channel| channel.is_configured())
        .collect();

    if configured.is_empty() {
        return Ok(Vec::new());
    }

    // Subscribe before starting gateways so immediate inbound events are not missed.
    let router = spawn_bot_session_router(
        Arc::clone(&registry),
        bus.clone(),
        base_system_prompt,
        provider_id,
        model,
    );

    let mut started = Vec::with_capacity(configured.len());
    for channel in configured {
        if let Err(err) = channel.start(bus.clone()).await {
            router.abort();
            return Err(err);
        }
        started.push(channel.id().to_string());
    }

    Ok(started)
}

/// Native Telegram Bot gateway using `RUVI_TELEGRAM_BOT_TOKEN` (and optional `RUVI_TELEGRAM_CHAT_ID`).
pub struct TelegramBotGateway {
    token: String,
    default_chat_id: Option<String>,
    client: Client,
}

impl TelegramBotGateway {
    /// Constructs [`TelegramBotGateway`] from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::with_credentials(
            std::env::var("RUVI_TELEGRAM_BOT_TOKEN").unwrap_or_default(),
            std::env::var("RUVI_TELEGRAM_CHAT_ID")
                .ok()
                .filter(|v| !v.trim().is_empty()),
        )
    }

    /// Constructs [`TelegramBotGateway`] with explicit credentials (useful for tests & plugins).
    #[must_use]
    pub fn with_credentials(token: impl Into<String>, default_chat_id: Option<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(35))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            token: token.into(),
            default_chat_id,
            client,
        }
    }
}

#[async_trait]
impl ChannelGateway for TelegramBotGateway {
    fn id(&self) -> &'static str {
        "telegram"
    }

    fn display_name(&self) -> &'static str {
        "Telegram Bot"
    }

    fn is_configured(&self) -> bool {
        !self.token.trim().is_empty()
    }

    async fn start(&self, bus: EventBus) -> CoreResult<()> {
        if !self.is_configured() {
            return Err(CoreError::Config(
                "set RUVI_TELEGRAM_BOT_TOKEN to enable Telegram bot gateway".to_string(),
            ));
        }
        let token = self.token.clone();
        let allowed_chat = self.default_chat_id.clone();
        let client = self.client.clone();

        tokio::spawn(async move {
            let mut offset: i64 = 0;
            loop {
                let url = format!(
                    "https://api.telegram.org/bot{token}/getUpdates?timeout=20&offset={offset}"
                );
                if let Ok(resp) = client.get(&url).send().await
                    && let Ok(parsed) = resp.json::<TgUpdatesResponse>().await
                {
                    for update in parsed.result {
                        offset = update.update_id + 1;
                        if let Some(msg) = update.message
                            && let Some(text) = msg.text
                        {
                            let chat_id = msg.chat.id.to_string();
                            if allowed_chat.as_deref().is_some_and(|id| id != chat_id) {
                                continue;
                            }
                            let sender = msg
                                .from
                                .and_then(|u| u.username.or(Some(u.first_name)))
                                .unwrap_or_else(|| "telegram-user".to_string());
                            let _ = bus.publish(AppEvent::ChannelMessageReceived {
                                channel_id: "telegram".to_string(),
                                destination: chat_id,
                                sender,
                                text,
                            });
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });

        Ok(())
    }

    async fn send_reply(&self, destination: &str, text: &str) -> CoreResult<()> {
        if !self.is_configured() {
            return Err(CoreError::Config(
                "Telegram bot token is not configured".to_string(),
            ));
        }
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.token);
        let resp = self
            .client
            .post(url)
            .json(&json!({ "chat_id": destination, "text": text }))
            .send()
            .await
            .map_err(|err| CoreError::Execution {
                component: self.id().to_string(),
                message: err.to_string(),
            })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoreError::Execution {
                component: self.id().to_string(),
                message: format!("Telegram sendMessage failed: {body}"),
            });
        }
        Ok(())
    }
}

/// Native Discord Bot gateway using `RUVI_DISCORD_BOT_TOKEN` and `RUVI_DISCORD_CHANNEL_ID`.
pub struct DiscordBotGateway {
    token: String,
    channel_id: String,
    client: Client,
}

impl DiscordBotGateway {
    /// Constructs [`DiscordBotGateway`] from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::with_credentials(
            std::env::var("RUVI_DISCORD_BOT_TOKEN").unwrap_or_default(),
            std::env::var("RUVI_DISCORD_CHANNEL_ID").unwrap_or_default(),
        )
    }

    /// Constructs [`DiscordBotGateway`] with explicit credentials.
    #[must_use]
    pub fn with_credentials(token: impl Into<String>, channel_id: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            token: token.into(),
            channel_id: channel_id.into(),
            client,
        }
    }
}

#[async_trait]
impl ChannelGateway for DiscordBotGateway {
    fn id(&self) -> &'static str {
        "discord"
    }

    fn display_name(&self) -> &'static str {
        "Discord Bot"
    }

    fn is_configured(&self) -> bool {
        !self.token.trim().is_empty() && !self.channel_id.trim().is_empty()
    }

    async fn start(&self, bus: EventBus) -> CoreResult<()> {
        if !self.is_configured() {
            return Err(CoreError::Config(
                "set RUVI_DISCORD_BOT_TOKEN and RUVI_DISCORD_CHANNEL_ID to enable Discord gateway"
                    .to_string(),
            ));
        }

        let token = self.token.clone();
        let channel_id = self.channel_id.clone();
        let client = self.client.clone();

        tokio::spawn(async move {
            let mut last_id: Option<String> = None;
            loop {
                let mut url =
                    format!("https://discord.com/api/v10/channels/{channel_id}/messages?limit=5");
                if let Some(after) = &last_id {
                    url.push_str("&after=");
                    url.push_str(after);
                }

                if let Ok(resp) = client
                    .get(&url)
                    .header("Authorization", format!("Bot {token}"))
                    .send()
                    .await
                    && let Ok(mut msgs) = resp.json::<Vec<DiscordMsg>>().await
                {
                    msgs.reverse();
                    for msg in msgs {
                        last_id = Some(msg.id);
                        if msg.author.bot.unwrap_or(false) || msg.content.trim().is_empty() {
                            continue;
                        }
                        let _ = bus.publish(AppEvent::ChannelMessageReceived {
                            channel_id: "discord".to_string(),
                            destination: channel_id.clone(),
                            sender: msg.author.username,
                            text: msg.content,
                        });
                    }
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });

        Ok(())
    }

    async fn send_reply(&self, destination: &str, text: &str) -> CoreResult<()> {
        if !self.is_configured() {
            return Err(CoreError::Config(
                "Discord bot token or channel ID is not configured".to_string(),
            ));
        }
        let target_channel = if destination.is_empty() {
            self.channel_id.as_str()
        } else {
            destination
        };
        let url = format!("https://discord.com/api/v10/channels/{target_channel}/messages");
        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("Bot {}", self.token))
            .json(&json!({ "content": text }))
            .send()
            .await
            .map_err(|err| CoreError::Execution {
                component: self.id().to_string(),
                message: err.to_string(),
            })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CoreError::Execution {
                component: self.id().to_string(),
                message: format!("Discord message failed: {body}"),
            });
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct TgUpdatesResponse {
    result: Vec<TgUpdate>,
}

#[derive(Deserialize)]
struct TgUpdate {
    update_id: i64,
    message: Option<TgMessage>,
}

#[derive(Deserialize)]
struct TgMessage {
    chat: TgChat,
    from: Option<TgUser>,
    text: Option<String>,
}

#[derive(Deserialize)]
struct TgChat {
    id: i64,
}

#[derive(Deserialize)]
struct TgUser {
    first_name: String,
    username: Option<String>,
}

#[derive(Deserialize)]
struct DiscordMsg {
    id: String,
    content: String,
    author: DiscordAuthor,
}

#[derive(Deserialize)]
struct DiscordAuthor {
    username: String,
    bot: Option<bool>,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use ruvi_core::{CompletionRequest, CompletionResponse, LlmProvider};

    use super::*;

    struct MockChannelGateway {
        id: &'static str,
        configured: bool,
        fail_start: bool,
        started: AtomicBool,
        sent: Arc<tokio::sync::Mutex<Vec<(String, String)>>>,
    }

    impl MockChannelGateway {
        fn new(id: &'static str, configured: bool) -> Self {
            Self {
                id,
                configured,
                fail_start: false,
                started: AtomicBool::new(false),
                sent: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl ChannelGateway for MockChannelGateway {
        fn id(&self) -> &str {
            self.id
        }

        fn display_name(&self) -> &'static str {
            "Mock Gateway"
        }

        fn is_configured(&self) -> bool {
            self.configured
        }

        async fn start(&self, _bus: EventBus) -> CoreResult<()> {
            if self.fail_start {
                return Err(CoreError::Config("mock start failure".to_string()));
            }
            self.started.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn send_reply(&self, destination: &str, text: &str) -> CoreResult<()> {
            self.sent
                .lock()
                .await
                .push((destination.to_string(), text.to_string()));
            Ok(())
        }
    }

    struct MockLlmProvider;

    #[async_trait]
    impl LlmProvider for MockLlmProvider {
        fn id(&self) -> &'static str {
            "mock-llm"
        }

        fn display_name(&self) -> &'static str {
            "Mock LLM"
        }

        async fn list_models(&self) -> CoreResult<Vec<String>> {
            Ok(vec!["mock-v1".to_string()])
        }

        async fn complete(&self, request: CompletionRequest) -> CoreResult<CompletionResponse> {
            assert_eq!(request.mode, AgentMode::Build);
            let sys = request
                .messages
                .first()
                .filter(|m| m.role == MessageRole::System)
                .map_or("", |m| m.content.as_str());
            let user_turns = request
                .messages
                .iter()
                .filter(|m| m.role == MessageRole::User)
                .count();
            let last_user = request
                .messages
                .iter()
                .rfind(|m| m.role == MessageRole::User)
                .map_or("", |m| m.content.as_str());

            if last_user.contains("FAIL_TURN") {
                return Err(CoreError::Provider {
                    provider: "mock-llm".to_string(),
                    message: "simulated failure".to_string(),
                });
            }

            let content = if last_user.contains("EMPTY_REPLY") {
                String::new()
            } else {
                format!("sys={sys} turns={user_turns} echo={last_user}")
            };

            Ok(CompletionResponse {
                model: request.model,
                content,
                tool_calls: Vec::new(),
            })
        }
    }

    async fn wait_for_sent_count(
        sent: &Arc<tokio::sync::Mutex<Vec<(String, String)>>>,
        expected: usize,
    ) -> Vec<(String, String)> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = sent.lock().await.clone();
                if snapshot.len() >= expected {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("timed out waiting for mock gateway send_reply")
    }

    #[test]
    fn gateways_report_configuration_accurately() {
        let tg_unconfigured = TelegramBotGateway::with_credentials("", None);
        let tg_configured = TelegramBotGateway::with_credentials("123:abc", None);
        assert!(!tg_unconfigured.is_configured());
        assert!(tg_configured.is_configured());

        let dc_partial = DiscordBotGateway::with_credentials("token", "");
        let dc_ready = DiscordBotGateway::with_credentials("token", "9999");
        assert!(!dc_partial.is_configured());
        assert!(dc_ready.is_configured());
    }

    #[tokio::test]
    async fn bot_session_router_replies_and_keeps_per_destination_history() {
        let gateway = Arc::new(MockChannelGateway::new("mock-tg", true));
        let sent = Arc::clone(&gateway.sent);

        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(MockLlmProvider));
        registry.register_channel(gateway);
        let registry = Arc::new(registry);
        let bus = EventBus::default();

        let router = spawn_bot_session_router(
            registry,
            bus.clone(),
            "base-prompt".to_string(),
            "mock-llm".to_string(),
            "mock-v1".to_string(),
        );

        let delivered = bus.publish(AppEvent::ChannelMessageReceived {
            channel_id: "mock-tg".to_string(),
            destination: "chat-1".to_string(),
            sender: "alice".to_string(),
            text: "hello ruvi".to_string(),
        });
        assert_eq!(delivered, 1);

        let first = wait_for_sent_count(&sent, 1).await;
        assert_eq!(first[0].0, "chat-1");
        assert!(first[0].1.contains("sys=base-prompt"));
        assert!(first[0].1.contains("turns=1"));
        assert!(first[0].1.contains("[alice] hello ruvi"));

        let _ = bus.publish(AppEvent::ChannelMessageReceived {
            channel_id: "mock-tg".to_string(),
            destination: "chat-1".to_string(),
            sender: "alice".to_string(),
            text: "follow up".to_string(),
        });
        let _ = bus.publish(AppEvent::ChannelMessageReceived {
            channel_id: "mock-tg".to_string(),
            destination: "chat-2".to_string(),
            sender: "bob".to_string(),
            text: "separate room".to_string(),
        });

        let all = wait_for_sent_count(&sent, 3).await;
        assert_eq!(all[1].0, "chat-1");
        assert!(all[1].1.contains("turns=2"));
        assert!(all[1].1.contains("follow up"));

        assert_eq!(all[2].0, "chat-2");
        assert!(all[2].1.contains("turns=1"));
        assert!(all[2].1.contains("[bob] separate room"));

        router.abort();
    }

    #[tokio::test]
    async fn start_configured_bots_starts_ready_gateways_and_routes_events() {
        let unconfigured = Arc::new(MockChannelGateway::new("mock-dc", false));
        let configured = Arc::new(MockChannelGateway::new("mock-tg", true));
        let sent = Arc::clone(&configured.sent);

        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(MockLlmProvider));
        registry.register_channel(Arc::clone(&unconfigured) as Arc<dyn ChannelGateway>);
        registry.register_channel(Arc::clone(&configured) as Arc<dyn ChannelGateway>);
        let registry = Arc::new(registry);
        let bus = EventBus::default();

        let started = start_configured_bots(
            registry,
            bus.clone(),
            "system".to_string(),
            "mock-llm".to_string(),
            "mock-v1".to_string(),
        )
        .await
        .expect("start_configured_bots should succeed");

        assert_eq!(started, vec!["mock-tg".to_string()]);
        assert!(configured.started.load(Ordering::SeqCst));
        assert!(!unconfigured.started.load(Ordering::SeqCst));

        let _ = bus.publish(AppEvent::ChannelMessageReceived {
            channel_id: "mock-tg".to_string(),
            destination: "room-42".to_string(),
            sender: "tester".to_string(),
            text: "ping bot".to_string(),
        });

        let replies = wait_for_sent_count(&sent, 1).await;
        assert_eq!(replies[0].0, "room-42");
        assert!(replies[0].1.contains("ping bot"));
    }

    #[tokio::test]
    async fn start_configured_bots_returns_empty_when_none_configured() {
        let mut registry = ExtensionRegistry::new();
        registry.register_channel(Arc::new(MockChannelGateway::new("mock-tg", false)));
        let started = start_configured_bots(
            Arc::new(registry),
            EventBus::default(),
            "system".to_string(),
            "mock-llm".to_string(),
            "mock-v1".to_string(),
        )
        .await
        .expect("should return Ok(empty)");
        assert_eq!(started, Vec::<String>::new());
    }

    #[tokio::test]
    async fn bot_session_router_skips_whitespace_handles_errors_and_empty_replies() {
        let gateway = Arc::new(MockChannelGateway::new("mock-tg", true));
        let sent = Arc::clone(&gateway.sent);

        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(MockLlmProvider));
        registry.register_channel(gateway);
        let bus = EventBus::default();

        let router = spawn_bot_session_router(
            Arc::new(registry),
            bus.clone(),
            "sys".to_string(),
            "mock-llm".to_string(),
            "mock-v1".to_string(),
        );

        let _ = bus.publish(AppEvent::ChannelMessageReceived {
            channel_id: "mock-tg".to_string(),
            destination: "chat-err".to_string(),
            sender: "alice".to_string(),
            text: "   \n ".to_string(),
        });
        let _ = bus.publish(AppEvent::ChannelMessageReceived {
            channel_id: "mock-tg".to_string(),
            destination: "chat-err".to_string(),
            sender: "alice".to_string(),
            text: "FAIL_TURN".to_string(),
        });
        let _ = bus.publish(AppEvent::ChannelMessageReceived {
            channel_id: "mock-tg".to_string(),
            destination: "chat-err".to_string(),
            sender: "alice".to_string(),
            text: "recovered".to_string(),
        });
        let _ = bus.publish(AppEvent::ChannelMessageReceived {
            channel_id: "mock-tg".to_string(),
            destination: "chat-err".to_string(),
            sender: "alice".to_string(),
            text: "EMPTY_REPLY".to_string(),
        });

        let replies = wait_for_sent_count(&sent, 2).await;
        assert!(replies[0].1.contains("turns=1"));
        assert!(replies[0].1.contains("recovered"));
        assert_eq!(replies[1].1, "(empty completion)");

        router.abort();
    }

    #[tokio::test]
    async fn bot_session_router_caps_history_at_sliding_window() {
        let gateway = Arc::new(MockChannelGateway::new("mock-tg", true));
        let sent = Arc::clone(&gateway.sent);

        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(MockLlmProvider));
        registry.register_channel(gateway);
        let bus = EventBus::default();

        let router = spawn_bot_session_router(
            Arc::new(registry),
            bus.clone(),
            "sys".to_string(),
            "mock-llm".to_string(),
            "mock-v1".to_string(),
        );

        let total_turns = 25usize;
        for idx in 0..total_turns {
            let _ = bus.publish(AppEvent::ChannelMessageReceived {
                channel_id: "mock-tg".to_string(),
                destination: "chat-cap".to_string(),
                sender: "alice".to_string(),
                text: format!("msg-{idx}"),
            });
        }

        let replies = wait_for_sent_count(&sent, total_turns).await;
        let last_reply = &replies[total_turns - 1].1;
        assert!(last_reply.contains("turns=21"));

        router.abort();
    }

    #[tokio::test]
    async fn start_configured_bots_aborts_router_when_gateway_start_fails() {
        let mut failing = MockChannelGateway::new("mock-tg", true);
        failing.fail_start = true;

        let mut registry = ExtensionRegistry::new();
        registry.register_provider(Arc::new(MockLlmProvider));
        registry.register_channel(Arc::new(failing));
        let bus = EventBus::default();

        let err = start_configured_bots(
            Arc::new(registry),
            bus.clone(),
            "sys".to_string(),
            "mock-llm".to_string(),
            "mock-v1".to_string(),
        )
        .await
        .expect_err("failing gateway start must return Err");
        assert!(matches!(err, CoreError::Config(_)));

        tokio::task::yield_now().await;
        assert_eq!(
            bus.publish(AppEvent::ChannelMessageReceived {
                channel_id: "mock-tg".to_string(),
                destination: "room".to_string(),
                sender: "u".to_string(),
                text: "after-abort".to_string(),
            }),
            0
        );
    }
}
