//! Thin wrapper over `webull_unofficial` for login, discovery, quotes, orders and basic order status.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tracing::{error, info};
use webull_unofficial::{
    error::WebullError,
    models::{OptionContract, OrderAction, Quote, TimeInForce},
    WebullClient,
};

use crate::types::Holding;

pub struct WbCtx {
    pub client: WebullClient,
    pub is_live: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrderStatus {
    Working,
    PartiallyFilled,
    Filled,
    Canceled,
    Rejected,
    Unknown(String),
}

#[derive(Debug, Clone)]
pub struct OrderInfo {
    pub status: OrderStatus,
    pub filled_qty: f64,
    pub avg_fill_price: f64,
}

impl WbCtx {
    /// Login using the crate's recommended builder style with an interactive MFA fallback.
    ///
    /// Flow:
    /// 1) Try builder login without MFA: `login_with().username(...).password(...).await`.
    /// 2) If server demands MFA or returns a generic auth error, prompt for code from CLI,
    ///    then login again with `.mfa(code)`.
    /// 3) If `mode == "live"`, fetch trade token via `get_trade_token(pin)`.
    pub async fn login(
        username: &str,
        password: &str,
        region: Option<i32>,
        mode: &str,
        trading_pin: Option<&str>,
    ) -> Result<Self> {
        // Create client per mode
        let mut client = match mode {
            "live" => WebullClient::new_live(region).context("create live client")?,
            _ => WebullClient::new_paper(region).context("create paper client")?,
        };

        info!(
            "Webull login attempt: user(partial)={}, mode={}, region={:?}",
            mask_user(username),
            mode,
            region
        );

        // First attempt: builder login without MFA (works on trusted device/IP)
        let first = client
            .login_with()
            .username(username)
            .password(password)
            .await;

        let mut need_mfa = false;
        match first {
            Ok(_) => {
                info!("Webull login success (no MFA required).");
            }
            Err(WebullError::MfaRequired) => {
                info!("MFA required by Webull.");
                need_mfa = true;
            }
            Err(WebullError::AuthenticationError(_)) => {
                // Some accounts/regions may return a generic AuthenticationError even when MFA is required.
                // We'll still try the MFA path interactively.
                error!("AuthenticationError on first attempt; will try interactive MFA.");
                need_mfa = true;
            }
            Err(e) => {
                error!("Webull login error: {:#?}", e);
                return Err(e).context("webull login failed");
            }
        }

        if need_mfa {
            let code = prompt_mfa("Enter the 6-digit Webull verification code: ").await?;
            client
                .login_with()
                .username(username)
                .password(password)
                .mfa(code.trim())
                .await
                .map_err(|e| {
                    error!("Webull login with MFA failed: {:#?}", e);
                    e
                })
                .context("webull login (with MFA) failed")?;
            info!("Webull login success (with MFA).");
        }

        // Live trading requires trade token (6-digit trading PIN)
        let is_live = mode == "live";
        if is_live {
            let pin = trading_pin.context("WEBULL_TRADING_PIN required for live")?;
            client
                .get_trade_token(pin)
                .await
                .context("get_trade_token failed")?;
            info!("Trade token acquired for live trading.");
        }

        Ok(Self { client, is_live })
    }

    // ---------- Discovery ----------

    pub async fn find_stock_ticker_id(&self, symbol: &str) -> Result<i64> {
        let found = self.client.find_ticker(symbol).await?;
        let first = found.first().context("no ticker found")?;
        Ok(first.ticker_id)
    }

    pub async fn find_option_contract(
        &self,
        symbol: &str,
        strike: f64,
        cp: char,
        expiry_mmdd: &str,
    ) -> Result<OptionContract> {
        let chain = self.client.get_options(symbol).await?;
        let want_mmdd = crate::utils::mmdd_digits(expiry_mmdd).context("bad MM/DD")?;
        let upper_cp = if cp.to_ascii_uppercase() == 'C' {
            "CALL"
        } else {
            "PUT"
        };
        let mut best: Option<OptionContract> = None;
        for c in chain.into_iter() {
            if (c.option_type.eq_ignore_ascii_case(upper_cp))
                && (c.strike_price - strike).abs() < 1e-6
            {
                if let (Some(a), Some(b)) = (
                    crate::utils::last4_digits(&c.expiration_date),
                    Some(want_mmdd.clone()),
                ) {
                    if a == b {
                        best = Some(c);
                        break;
                    }
                }
            }
        }
        best.context("option contract not found (by strike/type/expiry)")
    }

    // ---------- Quotes ----------

    pub async fn mid_price(&self, ticker_id: i64) -> Result<f64> {
        let q: Quote = self.client.get_quotes(&ticker_id.to_string()).await?;
        if let (Some(bid), Some(ask)) = (q.bid, q.ask) {
            if ask > 0.0 && bid > 0.0 {
                return Ok((bid + ask) / 2.0);
            }
        }
        Ok(q.close)
    }

    /// Return a simplified holdings snapshot parsed from Webull positions.
    pub async fn positions_simple(&self) -> Result<Vec<Holding>> {
        let raw_positions = self.client.get_positions().await?;
        let v: Value = serde_json::to_value(raw_positions)?;
        let mut out: Vec<Holding> = Vec::new();
        if let Some(arr) = v.as_array() {
            for it in arr {
                // Stock position
                if let Some(sym) = it
                    .get("ticker")
                    .and_then(|t| t.get("symbol"))
                    .and_then(|s| s.as_str())
                {
                    let qty = it
                        .get("position")
                        .and_then(|x| x.as_f64())
                        .or_else(|| {
                            it.get("position")
                                .and_then(|x| x.as_i64().map(|n| n as f64))
                        })
                        .unwrap_or(0.0);
                    let avg = it
                        .get("cost")
                        .or_else(|| it.get("avgPrice"))
                        .and_then(|x| x.as_f64())
                        .unwrap_or(0.0);
                    out.push(Holding::Stock {
                        symbol: sym.to_string(),
                        quantity: qty,
                        avg_cost: avg,
                    });
                    continue;
                }
                // Option position
                let underlying = it
                    .get("underlyingSymbol")
                    .or_else(|| it.get("symbol"))
                    .and_then(|s| s.as_str());
                let strike = it
                    .get("strikePrice")
                    .or_else(|| it.get("strike"))
                    .and_then(|x| x.as_f64());
                let cp = it
                    .get("callOrPut")
                    .or_else(|| it.get("putCall"))
                    .and_then(|s| s.as_str())
                    .and_then(|s| s.chars().next());
                let exp_raw = it
                    .get("expireDate")
                    .or_else(|| it.get("expirationDate"))
                    .or_else(|| it.get("expire_date"))
                    .and_then(|s| s.as_str());
                let qty = it.get("position").and_then(|x| x.as_i64()).unwrap_or(0) as u32;
                let avg = it
                    .get("cost")
                    .or_else(|| it.get("avgPrice"))
                    .and_then(|x| x.as_f64())
                    .unwrap_or(0.0);
                if let (Some(under), Some(strk), Some(cp_ch), Some(exp)) =
                    (underlying, strike, cp, exp_raw)
                {
                    let mmdd =
                        crate::utils::last4_digits(exp).unwrap_or_else(|| "0000".to_string());
                    out.push(Holding::Option {
                        symbol: under.to_string(),
                        strike: strk,
                        call_put: cp_ch.to_ascii_uppercase(),
                        expiry_mmdd: mmdd,
                        quantity: qty,
                        avg_cost: avg,
                    });
                }
            }
        }
        Ok(out)
    }

    // ---------- Order status & actions ----------

    pub async fn get_order_info(&self, order_id: &str) -> Result<OrderInfo> {
        // Use get_orders(None) and filter locally
        let arr = self.client.get_orders(None).await?;
        let vv: Value = serde_json::to_value(arr)?;
        let v = vv
            .as_array()
            .and_then(|a| {
                a.iter()
                    .find(|it| {
                        // orderId could be string or number; try common aliases too
                        let oid = it
                            .get("orderId")
                            .and_then(|x| match x {
                                Value::String(s) => Some(s.clone()),
                                Value::Number(n) => Some(n.to_string()),
                                _ => None,
                            })
                            .or_else(|| {
                                it.get("order_id")
                                    .and_then(|x| x.as_str().map(|s| s.to_string()))
                            })
                            .or_else(|| {
                                it.get("orderIdStr")
                                    .and_then(|x| x.as_str().map(|s| s.to_string()))
                            });
                        matches!(oid, Some(ref s) if s == order_id)
                    })
                    .cloned()
            })
            .unwrap_or(Value::Null);

        // Status mapping
        let status_str = v
            .get("status")
            .or_else(|| v.get("orderStatus"))
            .and_then(|s| s.as_str())
            .unwrap_or("UNKNOWN")
            .to_string();

        let status = match status_str.to_ascii_uppercase().as_str() {
            "WORKING" | "OPEN" | "PENDING" => OrderStatus::Working,
            "PARTIALLY_FILLED" | "PARTIAL" => OrderStatus::PartiallyFilled,
            "FILLED" => OrderStatus::Filled,
            "CANCELED" | "CANCELLED" => OrderStatus::Canceled,
            "REJECTED" => OrderStatus::Rejected,
            other => OrderStatus::Unknown(other.to_string()),
        };

        // Fills
        let filled_qty = v
            .get("filledQuantity")
            .or_else(|| v.get("filledQty"))
            .or_else(|| v.get("filled_quantity"))
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);

        let avg_fill_price = v
            .get("filledAvgPrice")
            .or_else(|| v.get("avgFillPrice"))
            .or_else(|| v.get("avg_fill_price"))
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);

        Ok(OrderInfo {
            status,
            filled_qty,
            avg_fill_price,
        })
    }

    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        self.client.cancel_order(order_id).await?;
        Ok(())
    }

    // ---------- Orders (Stocks) ----------

    pub async fn place_stock_market(
        &self,
        symbol: &str,
        qty: f64,
        side: OrderAction,
        tif: &TimeInForce,
    ) -> Result<String> {
        let tid = self.find_stock_ticker_id(symbol).await?;
        let order_id = self
            .client
            .place_market_order_with()
            .ticker_id(tid)
            .quantity(qty)
            .action(side)
            .time_in_force(tif.clone())
            .await?;
        Ok(order_id)
    }

    pub async fn place_stock_limit(
        &self,
        symbol: &str,
        qty: f64,
        side: OrderAction,
        limit: f64,
        tif: &TimeInForce,
    ) -> Result<String> {
        let tid = self.find_stock_ticker_id(symbol).await?;
        let order_id = self
            .client
            .place_limit_order_with(limit)
            .ticker_id(tid)
            .quantity(qty)
            .action(side)
            .time_in_force(tif.clone())
            .await?;
        Ok(order_id)
    }

    // ---------- Orders (Options) ----------

    pub async fn place_option_market(
        &self,
        contract: &OptionContract,
        qty: f64,
        side: OrderAction,
        tif: &TimeInForce,
    ) -> Result<String> {
        let order_id = self
            .client
            .place_market_order_with()
            .ticker_id(contract.ticker_id)
            .quantity(qty)
            .action(side)
            .time_in_force(tif.clone())
            .await?;
        Ok(order_id)
    }

    pub async fn place_option_limit(
        &self,
        contract: &OptionContract,
        qty: f64,
        side: OrderAction,
        limit: f64,
        tif: &TimeInForce,
    ) -> Result<String> {
        let order_id = self
            .client
            .place_limit_order_with(limit)
            .ticker_id(contract.ticker_id)
            .quantity(qty)
            .action(side)
            .time_in_force(tif.clone())
            .await?;
        Ok(order_id)
    }
}

/// Prompt MFA code from CLI using a blocking read on a dedicated blocking thread.
async fn prompt_mfa(prompt: &str) -> Result<String> {
    use std::io::{self, Write};
    let prompt = prompt.to_string();
    let code = tokio::task::spawn_blocking(move || -> Result<String> {
        print!("{}", prompt);
        let _ = io::stdout().flush();
        let mut buf = String::new();
        io::stdin().read_line(&mut buf)?;
        let s = buf.trim().to_string();
        if s.is_empty() {
            return Err(anyhow!("Empty MFA code"));
        }
        Ok(s)
    })
    .await
    .map_err(|e| anyhow!("spawn_blocking join error: {}", e))??;
    Ok(code)
}

/// Print first two chars, then mask the rest (for logs only).
fn mask_user(u: &str) -> String {
    let mut cs = u.chars();
    let a = cs.next().unwrap_or('*');
    let b = cs.next().unwrap_or('*');
    format!("{}{}****", a, b)
}
