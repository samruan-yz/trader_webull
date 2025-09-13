//! Serenity-self based Discord listener (self-bot). Filters channel and tracked users.

use serenity_self::all::{Client, EventHandler, GatewayIntents, Message};
use serenity_self::async_trait;
use tracing::{info, warn};

use crate::parser::parse_signal;
use crate::types::TradeSignal;

pub struct Handler {
    pub channel_ids: Vec<String>,
    pub tracked_users: Vec<String>,
    pub tx: tokio::sync::mpsc::Sender<(String, TradeSignal)>, // (author, parsed signal)
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, _ctx: serenity_self::all::Context, msg: Message) {
        // Channel filter (multiple)
        let ch = msg.channel_id.get().to_string();
        if !self.channel_ids.iter().any(|id| id == &ch) {
            return;
        }
        // Author filter (fuzzy, case-insensitive substring)
        let author_name = msg.author.name.clone();
        let author_lower = author_name.to_lowercase();
        let auth_ok = self
            .tracked_users
            .iter()
            .map(|s| s.to_lowercase())
            .any(|needle| author_lower.contains(&needle));
        if !auth_ok {
            return;
        }

        let content = msg.content.clone();
        match parse_signal(&content) {
            Some(sig) => {
                let _ = self.tx.send((author_name, sig)).await;
            }
            None => {
                warn!("Unrecognized signal: {}", content);
            }
        }
    }
}

pub async fn run(
    token: &str,
    channel_ids: Vec<String>,
    tracked_users: Vec<String>,
    tx: tokio::sync::mpsc::Sender<(String, TradeSignal)>,
) -> anyhow::Result<()> {
    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;
    let handler = Handler {
        channel_ids,
        tracked_users,
        tx,
    };

    let mut client = Client::builder(token, intents)
        .event_handler(handler)
        .await?;

    info!("Discord self-bot starting...");
    client.start().await?;
    Ok(())
}
