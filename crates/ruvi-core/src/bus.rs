use tokio::sync::broadcast;

use crate::domain::{AgentMode, SessionId};

/// Broadcast events published across decoupled vertical slices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    /// Agent execution mode switched between Build and Plan.
    ModeChanged {
        /// Active session identifier.
        session_id: SessionId,
        /// New mode.
        mode: AgentMode,
    },
    /// Active provider or model changed.
    ModelSelected {
        /// Provider identifier.
        provider_id: String,
        /// Selected model identifier.
        model: String,
    },
    /// User submitted a prompt in a session or channel.
    PromptSubmitted {
        /// Session identifier.
        session_id: SessionId,
        /// Prompt text.
        text: String,
    },
    /// External messaging channel (Telegram, Discord, etc.) received an inbound prompt.
    ChannelMessageReceived {
        /// Gateway identifier (`telegram`, `discord`, etc.).
        channel_id: String,
        /// External chat/channel destination ID.
        destination: String,
        /// Sender display name or ID.
        sender: String,
        /// Message text.
        text: String,
    },
    /// A local runtime installation or model pull completed.
    RuntimeUpdated {
        /// Runtime manager ID.
        runtime_id: String,
        /// Status summary message.
        summary: String,
    },
}

/// Asynchronous pub/sub event bus decouples slices from direct dependencies on each other.
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<AppEvent>,
}

impl EventBus {
    /// Creates an [`EventBus`] with the given bounded channel capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self { sender }
    }

    /// Publishes an [`AppEvent`] to all active subscribers.
    ///
    /// Returns the number of active receivers that received the event (0 if none).
    #[must_use]
    pub fn publish(&self, event: AppEvent) -> usize {
        self.sender.send(event).unwrap_or(0)
    }

    /// Creates a new [`broadcast::Receiver`] subscribed to future events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.sender.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(128)
    }
}
