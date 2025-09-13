//! Risk checks before order placement (V2).

use crate::state::BotState;
use crate::types::{Action, TradeSignal};
use anyhow::Result;

pub struct RiskEngine {
    max_position_value: f64,
}

impl RiskEngine {
    pub fn new(max_value: f64) -> Self {
        Self {
            max_position_value: max_value,
        }
    }

    pub fn pre_check(&self, signal: &TradeSignal, est_price: f64, state: &BotState) -> Result<()> {
        let notional = match signal {
            TradeSignal::Stock(s) => est_price * (s.quantity as f64),
            TradeSignal::Option(o) => est_price * (o.quantity as f64) * 100.0,
        };
        if notional > self.max_position_value {
            anyhow::bail!(
                "Order notional ${:.2} exceeds max_position_value ${:.2}",
                notional,
                self.max_position_value
            );
        }
        match signal {
            TradeSignal::Stock(s) if s.action == Action::STC => {
                let have = state.position_qty_stock(&s.symbol);
                if have + 1e-9 < s.quantity as f64 {
                    anyhow::bail!(
                        "Cannot STC {} shares of {}: holding {:.4}",
                        s.quantity,
                        s.symbol,
                        have
                    );
                }
            }
            TradeSignal::Option(o) if o.action == Action::STC => {
                let have =
                    state.position_qty_option(&o.symbol, o.strike, o.call_put, &o.expiry_mmdd);
                if have < o.quantity {
                    anyhow::bail!(
                        "Cannot STC {}x {} {}{} {}: holding {}",
                        o.quantity,
                        o.symbol,
                        o.strike,
                        o.call_put,
                        o.expiry_mmdd,
                        have
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }
}
