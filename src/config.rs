//! Load and validate runtime configuration.

use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Debug, Deserialize, Clone)]
pub struct DiscordCfg {
    pub channel_ids: Vec<String>,
    pub tracked_users: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WebullCfg {
    pub region: Option<i32>, // e.g., 6 for US
    pub mode: String,        // "paper" or "live"
}

#[derive(Debug, Deserialize, Clone)]
pub struct RiskCfg {
    pub max_position_value: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExecCfg {
    pub dry_run: bool,
    pub tif: String, // "DAY" or "GTC"

    // New controls
    pub buy_mode: String,  // "LIMIT" | "MARKET"
    pub sell_mode: String, // "LIMIT" | "MARKET"
    pub buy_timeout_sec: u64,
    pub sell_timeout_sec: u64,
    pub buy_limit_slippage_pct: f64,
    pub sell_limit_slippage_pct: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StateCfg {
    pub path: String,
    pub flush_interval_sec: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub discord: DiscordCfg,
    pub webull: WebullCfg,
    pub risk: RiskCfg,
    pub exec: ExecCfg,
    pub state: StateCfg,
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let s = fs::read_to_string(path)?;
        let cfg: Self = serde_yaml::from_str(&s)?;
        Ok(cfg)
    }
}
