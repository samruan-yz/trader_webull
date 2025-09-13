//! Persisted bot state. V3: store full holdings and realized daily P/L entries.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

use crate::types::{Holding, PlEntry};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BotState {
    /// Full current holdings snapshot (stocks & options).
    pub holdings: Vec<Holding>,
    /// Realized P/L entries by day.
    pub daily_pl: Vec<PlEntry>,
}

impl BotState {
    pub fn load(path: &str) -> Self {
        if Path::new(path).exists() {
            if let Ok(s) = fs::read_to_string(path) {
                if let Ok(me) = serde_json::from_str::<Self>(&s) {
                    return me;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self, path: &str) -> anyhow::Result<()> {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = serde_json::to_string_pretty(self)?;
        fs::write(path, s)?;
        Ok(())
    }

    pub fn set_holdings(&mut self, new_holdings: Vec<Holding>) {
        self.holdings = new_holdings;
    }

    pub fn position_qty_stock(&self, symbol: &str) -> f64 {
        let sym = symbol.to_ascii_uppercase();
        self.holdings.iter().fold(0.0, |acc, h| match h {
            Holding::Stock {
                symbol, quantity, ..
            } if symbol.eq_ignore_ascii_case(&sym) => acc + *quantity,
            _ => acc,
        })
    }

    pub fn position_qty_option(
        &self,
        symbol: &str,
        strike: f64,
        cp: char,
        expiry_mmdd: &str,
    ) -> u32 {
        let sym = symbol.to_ascii_uppercase();
        let cp_u = cp.to_ascii_uppercase();
        self.holdings.iter().fold(0u32, |acc, h| match h {
            Holding::Option {
                symbol,
                strike: s,
                call_put,
                expiry_mmdd,
                quantity,
                ..
            } if symbol.eq_ignore_ascii_case(&sym)
                && (*s - strike).abs() < 1e-6
                && call_put.to_ascii_uppercase() == cp_u
                && expiry_mmdd == expiry_mmdd =>
            {
                acc + *quantity
            }
            _ => acc,
        })
    }

    /// Weighted-average add for stock BUY fills.
    pub fn upsert_stock_buy_with_cost(&mut self, symbol: &str, fill_qty: f64, fill_price: f64) {
        let sym = symbol.to_ascii_uppercase();
        if let Some(h) = self.holdings.iter_mut().find(
            |h| matches!(h, Holding::Stock { symbol, .. } if symbol.eq_ignore_ascii_case(&sym)),
        ) {
            if let Holding::Stock {
                quantity, avg_cost, ..
            } = h
            {
                let total_cost = *avg_cost * *quantity + fill_price * fill_qty;
                *quantity += fill_qty;
                *avg_cost = if *quantity > 0.0 {
                    total_cost / *quantity
                } else {
                    0.0
                };
            }
        } else {
            self.holdings.push(Holding::Stock {
                symbol: sym,
                quantity: fill_qty,
                avg_cost: fill_price,
            });
        }
    }

    /// Weighted-average add for option BUY fills.
    pub fn upsert_option_buy_with_cost(
        &mut self,
        symbol: &str,
        strike: f64,
        cp: char,
        expiry_mmdd: &str,
        fill_qty: u32,
        fill_price: f64,
    ) {
        let sym = symbol.to_ascii_uppercase();
        let cp_u = cp.to_ascii_uppercase();
        let exp = expiry_mmdd.to_string();
        if let Some(h) = self.holdings.iter_mut().find(|h| {
            matches!(h, Holding::Option { symbol, strike: s, call_put, expiry_mmdd, .. }
                if symbol.eq_ignore_ascii_case(&sym) && (*s - strike).abs() < 1e-6 && call_put.to_ascii_uppercase() == cp_u && expiry_mmdd == &exp)
        }) {
            if let Holding::Option { quantity, avg_cost, .. } = h {
                let qf = *quantity as f64; let total_cost = *avg_cost * qf + fill_price * (fill_qty as f64);
                *quantity += fill_qty; *avg_cost = if *quantity > 0 { total_cost / (*quantity as f64) } else { 0.0 };
            }
        } else {
            self.holdings.push(Holding::Option { symbol: sym, strike, call_put: cp_u, expiry_mmdd: exp, quantity: fill_qty, avg_cost: fill_price });
        }
    }

    /// Realize P/L for stock sell; decrease position by qty. Returns realized P/L.
    pub fn realize_stock_sell(
        &mut self,
        symbol: &str,
        sell_qty: f64,
        sell_price: f64,
        date: NaiveDate,
    ) -> f64 {
        let sym = symbol.to_ascii_uppercase();
        let mut realized = 0.0;
        let mut remove_idx: Option<usize> = None;
        for (i, h) in self.holdings.iter_mut().enumerate() {
            if let Holding::Stock {
                symbol,
                quantity,
                avg_cost,
            } = h
            {
                if symbol.eq_ignore_ascii_case(&sym) {
                    let q = sell_qty.min(*quantity);
                    realized = (sell_price - *avg_cost) * q;
                    *quantity -= q;
                    if *quantity <= 1e-9 {
                        remove_idx = Some(i);
                    }
                    self.daily_pl.push(PlEntry {
                        date,
                        asset: sym.clone(),
                        qty: q,
                        realized_pl: realized,
                    });
                    break;
                }
            }
        }
        if let Some(i) = remove_idx {
            self.holdings.remove(i);
        }
        realized
    }

    /// Realize P/L for option sell; decrease position by contracts. Returns realized P/L.
    pub fn realize_option_sell(
        &mut self,
        symbol: &str,
        strike: f64,
        cp: char,
        expiry_mmdd: &str,
        sell_qty: u32,
        sell_price: f64,
        date: NaiveDate,
    ) -> f64 {
        let sym = symbol.to_ascii_uppercase();
        let cp_u = cp.to_ascii_uppercase();
        let mut realized = 0.0;
        let mut remove_idx: Option<usize> = None;
        for (i, h) in self.holdings.iter_mut().enumerate() {
            if let Holding::Option {
                symbol,
                strike: s,
                call_put,
                expiry_mmdd: exp,
                quantity,
                avg_cost,
            } = h
            {
                if symbol.eq_ignore_ascii_case(&sym)
                    && (*s - strike).abs() < 1e-6
                    && call_put.to_ascii_uppercase() == cp_u
                    && exp == expiry_mmdd
                {
                    let q = sell_qty.min(*quantity);
                    // Options PL is per contract × 100 shares
                    realized = (sell_price - *avg_cost) * (q as f64) * 100.0;
                    *quantity -= q;
                    if *quantity == 0 {
                        remove_idx = Some(i);
                    }
                    let asset = format!("{} {}{} {}", sym, strike, cp_u, expiry_mmdd);
                    self.daily_pl.push(PlEntry {
                        date,
                        asset,
                        qty: q as f64,
                        realized_pl: realized,
                    });
                    break;
                }
            }
        }
        if let Some(i) = remove_idx {
            self.holdings.remove(i);
        }
        realized
    }
}
