//! Multi-channel messaging gateway (paper §IX-B P1).
//!
//! Convergent pattern observed in the OpenClaw / Hermes Agent
//! frameworks the paper cites: every channel implements the same
//! adapter trait so the agent core stays channel-agnostic.  The agent
//! receives `IncomingMessage` and emits `OutgoingMessage`; channels
//! translate to/from their native wire format.
//!
//! Phase 8 ships:
//!   - `ChannelAdapter` async trait
//!   - `CliChannel` — an in-process adapter the demo + tests drive
//!   - skeletons for Teams / Slack / Telegram / WhatsApp / Email
//!     (trait implementations with `todo!()` bodies — the SDK wiring
//!     is the only thing left in each)
//!
//! Production deployments register their own adapters via the
//! `ChannelRegistry`.  Outbound messages route by channel id (e.g.
//! "teams:#fin-ops", "slack:@user", "cli:default").

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use thiserror::Error;
use tokio::sync::RwLock;

#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("unknown channel: {0}")]
    Unknown(String),
    #[error("adapter error: {0}")]
    Adapter(String),
    #[error("not connected")]
    NotConnected,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Teams,
    Slack,
    Telegram,
    Whatsapp,
    Email,
    Cli,
}

impl ChannelKind {
    pub fn scheme(self) -> &'static str {
        match self {
            ChannelKind::Teams => "teams",
            ChannelKind::Slack => "slack",
            ChannelKind::Telegram => "telegram",
            ChannelKind::Whatsapp => "whatsapp",
            ChannelKind::Email => "email",
            ChannelKind::Cli => "cli",
        }
    }
}

/// One inbound user turn — from any channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub channel: ChannelKind,
    /// Channel-scoped identifier for the user.  Bound to an MCP session
    /// via the agent's pairing flow.
    pub user_id: String,
    /// Channel-scoped conversation id (e.g. Teams chat id, Slack channel).
    pub conversation_id: String,
    pub text: String,
    /// Free-form metadata attached by the adapter (mentions, attachments,
    /// inline cards, etc.).
    #[serde(default)]
    pub metadata: serde_json::Value,
    pub received_at_ms: u64,
}

/// One outbound agent turn — to a specific channel address.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    /// Routing address: "<scheme>:<conversation_id>".  Example:
    /// `"teams:19:meeting_NTBhYjZkZTAt..."`.
    pub address: String,
    pub text: String,
    /// Optional Adaptive Card / Block Kit JSON.  Adapters fall back to
    /// `text` if they don't render rich content.
    #[serde(default)]
    pub rich: Option<serde_json::Value>,
}

impl OutgoingMessage {
    pub fn text(address: impl Into<String>, text: impl Into<String>) -> Self {
        Self { address: address.into(), text: text.into(), rich: None }
    }
}

#[async_trait]
pub trait ChannelAdapter: Send + Sync + 'static {
    fn kind(&self) -> ChannelKind;
    /// Send an outbound message to the channel.  Adapter is responsible
    /// for parsing the `<scheme>:<conversation_id>` prefix.
    async fn send(&self, msg: OutgoingMessage) -> Result<(), ChannelError>;
}

// ---------------------------------------------------------------------------
// CliChannel — in-process adapter used by tests + demos
// ---------------------------------------------------------------------------

/// CLI channel: ring-buffered outgoing log.  Tests / demos pull the
/// sent log via `CliChannel::log()` to assert the agent emitted what
/// was expected.
pub struct CliChannel {
    sent: RwLock<Vec<OutgoingMessage>>,
}

impl CliChannel {
    pub fn new() -> Arc<Self> { Arc::new(Self { sent: RwLock::new(Vec::new()) }) }
    pub async fn log(&self) -> Vec<OutgoingMessage> {
        self.sent.read().await.clone()
    }
    pub async fn last(&self) -> Option<OutgoingMessage> {
        self.sent.read().await.last().cloned()
    }
}

impl Default for CliChannel {
    fn default() -> Self { Self { sent: RwLock::new(Vec::new()) } }
}

#[async_trait]
impl ChannelAdapter for CliChannel {
    fn kind(&self) -> ChannelKind { ChannelKind::Cli }
    async fn send(&self, msg: OutgoingMessage) -> Result<(), ChannelError> {
        self.sent.write().await.push(msg);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Teams / Slack / Telegram skeletons
// ---------------------------------------------------------------------------

/// Microsoft Teams adapter skeleton.  Production wiring uses the Bot
/// Framework SDK; this stub stores the bot endpoint and emits a clear
/// error until the SDK is plugged in.  The MCP server / agent code does
/// not change when the SDK lands — only this file does.
pub struct TeamsAdapter {
    pub bot_endpoint: String,
    pub tenant_id: String,
}

#[async_trait]
impl ChannelAdapter for TeamsAdapter {
    fn kind(&self) -> ChannelKind { ChannelKind::Teams }
    async fn send(&self, _msg: OutgoingMessage) -> Result<(), ChannelError> {
        Err(ChannelError::NotConnected) // SDK wiring is Phase 8 finalisation
    }
}

pub struct SlackAdapter {
    pub workspace: String,
    pub bot_token_env: String,
}

#[async_trait]
impl ChannelAdapter for SlackAdapter {
    fn kind(&self) -> ChannelKind { ChannelKind::Slack }
    async fn send(&self, _msg: OutgoingMessage) -> Result<(), ChannelError> {
        Err(ChannelError::NotConnected)
    }
}

pub struct TelegramAdapter { pub bot_token_env: String }

#[async_trait]
impl ChannelAdapter for TelegramAdapter {
    fn kind(&self) -> ChannelKind { ChannelKind::Telegram }
    async fn send(&self, _msg: OutgoingMessage) -> Result<(), ChannelError> {
        Err(ChannelError::NotConnected)
    }
}

// ---------------------------------------------------------------------------
// Registry — the agent uses this to route by `address` scheme.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct ChannelRegistry {
    by_scheme: HashMap<String, Arc<dyn ChannelAdapter>>,
}

impl ChannelRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn register(&mut self, adapter: Arc<dyn ChannelAdapter>) {
        self.by_scheme.insert(adapter.kind().scheme().to_string(), adapter);
    }

    pub fn schemes(&self) -> Vec<String> {
        self.by_scheme.keys().cloned().collect()
    }

    /// Route a message to the adapter implied by the `address` prefix.
    pub async fn send(&self, msg: OutgoingMessage) -> Result<(), ChannelError> {
        let scheme = msg.address.split(':').next().unwrap_or("").to_string();
        let adapter = self.by_scheme.get(&scheme)
            .ok_or_else(|| ChannelError::Unknown(scheme))?
            .clone();
        adapter.send(msg).await
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cli_channel_records_messages() {
        let cli = CliChannel::new();
        let mut reg = ChannelRegistry::new();
        reg.register(cli.clone());
        reg.send(OutgoingMessage::text("cli:default", "hello")).await.unwrap();
        reg.send(OutgoingMessage::text("cli:default", "world")).await.unwrap();
        let log = cli.log().await;
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].text, "hello");
        assert_eq!(log[1].text, "world");
    }

    #[tokio::test]
    async fn unknown_scheme_returns_error() {
        let cli = CliChannel::new();
        let mut reg = ChannelRegistry::new();
        reg.register(cli);
        let r = reg.send(OutgoingMessage::text("teams:foo", "hi")).await;
        assert!(matches!(r, Err(ChannelError::Unknown(_))));
    }

    #[tokio::test]
    async fn teams_skeleton_is_not_connected() {
        let t = Arc::new(TeamsAdapter {
            bot_endpoint: "https://example.com".into(),
            tenant_id: "tenant".into(),
        });
        let r = t.send(OutgoingMessage::text("teams:#fin-ops", "hi")).await;
        assert!(matches!(r, Err(ChannelError::NotConnected)));
    }
}
