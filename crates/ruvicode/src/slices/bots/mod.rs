use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use ruvi_core::{AppEvent, ChannelGateway, CoreError, CoreResult, EventBus};
use serde::Deserialize;
use serde_json::json;

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
    use super::*;

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
}
