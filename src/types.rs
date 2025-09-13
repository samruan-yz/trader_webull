//! Core domain types for signals, orders, holdings and realized P/L.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Action {
    BTO,
    STC,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AssetKind {
    Stock,
    Option,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl From<Action> for Side {
    fn from(a: Action) -> Self {
        match a {
            Action::BTO => Side::Buy,
            Action::STC => Side::Sell,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockSignal {
    pub action: Action,
    pub symbol: String,
    pub quantity: u32,
    pub order_type: OrderType,
    pub limit_price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionSignal {
    pub action: Action,
    pub symbol: String,
    pub strike: f64,
    pub call_put: char, // 'C' or 'P'
    pub expiry_mmdd: String,
    pub quantity: u32,
    pub order_type: OrderType,
    pub limit_price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TradeSignal {
    Stock(StockSignal),
    Option(OptionSignal),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Holding {
    /// Stock holding with average cost per share.
    Stock {
        symbol: String,
        quantity: f64,
        avg_cost: f64,
    },
    /// Option holding with average premium per contract.
    Option {
        symbol: String,
        strike: f64,
        call_put: char,
        expiry_mmdd: String,
        quantity: u32,
        avg_cost: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlEntry {
    pub date: NaiveDate,
    pub asset: String,    // e.g., "AAPL" or "AAPL 150C 08/16"
    pub qty: f64,         // shares or contracts
    pub realized_pl: f64, // USD; options already ×100 accounted where recorded
}
