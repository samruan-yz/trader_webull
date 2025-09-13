//! Entry point. Wires Discord -> Parser -> Risk -> Webull.

mod config;
mod discord;
mod parser;
mod risk;
mod state;
mod types;
mod utils;
mod webull_client;

use dotenvy::dotenv;
use tracing::{error, info, Level};
use tracing_subscriber::EnvFilter;

use crate::types::{Action, OrderType, TradeSignal};
use crate::utils::{sanitize_symbol, tif_from_str};
use chrono::Local;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
use webull_client::{OrderInfo, OrderStatus};
use webull_unofficial::models::{OrderAction, TimeInForce};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(Level::INFO.into()))
        .init();

    // Load config
    let cfg = config::AppConfig::load("config.yaml")?;
    let discord_token = std::env::var("DISCORD_USER_TOKEN")?;
    let wb_user = std::env::var("WEBULL_USERNAME")?;
    let wb_pass = std::env::var("WEBULL_PASSWORD")?;
    let wb_pin = std::env::var("WEBULL_TRADING_PIN").ok(); // live only

    // State & Risk (state -> Arc<Mutex<...>> for concurrent monitor tasks)
    let state = Arc::new(Mutex::new(state::BotState::load(&cfg.state.path)));
    let risk = risk::RiskEngine::new(cfg.risk.max_position_value);

    // Webull login (paper/live) -> Arc
    let wb = Arc::new(
        webull_client::WbCtx::login(
            &wb_user,
            &wb_pass,
            cfg.webull.region,
            &cfg.webull.mode,
            wb_pin.as_deref(),
        )
        .await?,
    );
    info!("Webull mode: {}", if wb.is_live { "live" } else { "paper" });

    // Initial holdings sync (once at startup)
    match wb.positions_simple().await {
        Ok(holdings) => {
            let mut st = state.lock().await;
            st.set_holdings(holdings);
            let _ = st.save(&cfg.state.path);
            info!("Initial holdings synced from Webull");
        }
        Err(e) => error!("Initial holdings sync failed: {:#}", e),
    }

    // Discord channel -> internal MPSC
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(String, TradeSignal)>(1024);
    let discord_handle = tokio::spawn({
        let token = discord_token.clone();
        let dcfg = cfg.discord.clone();
        async move {
            if let Err(e) = discord::run(&token, dcfg.channel_ids, dcfg.tracked_users, tx).await {
                error!("Discord run error: {:#}", e);
            }
        }
    });

    let tif: TimeInForce = tif_from_str(&cfg.exec.tif);
    info!(
        "Trader started. Mode={}, TIF={:?}, DryRun={}, SyncEvery={}s, buy/sell mode = {}/{}",
        cfg.webull.mode,
        tif,
        cfg.exec.dry_run,
        cfg.state.flush_interval_sec,
        cfg.exec.buy_mode,
        cfg.exec.sell_mode
    );

    // Periodic holdings sync ticker
    let mut sync_ticker = tokio::time::interval(Duration::from_secs(cfg.state.flush_interval_sec));

    // Helpers to choose effective order mode and compute limit with slippage
    let buy_is_market = cfg.exec.buy_mode.eq_ignore_ascii_case("MARKET");
    let sell_is_market = cfg.exec.sell_mode.eq_ignore_ascii_case("MARKET");

    loop {
        tokio::select! {
            maybe = rx.recv() => {
                let Some((author, signal)) = maybe else { break; };
                info!("Signal from {}: {:?}", author, signal);

                match signal {
                    TradeSignal::Stock(s) => {
                        let symbol = sanitize_symbol(&s.symbol);
                        let tid = match wb.find_stock_ticker_id(&symbol).await {
                            Ok(v) => v,
                            Err(e) => { error!("find stock ticker failed: {:#}", e); continue; }
                        };

                        // Base price for risk & possible derived limit when needed
                        let mut est_price = if let (OrderType::Limit, Some(p)) = (s.order_type, s.limit_price) {
                            p
                        } else {
                            wb.mid_price(tid).await.unwrap_or(0.0)
                        };

                        // risk check reads state under lock
                        {
                            let st = state.lock().await;
                            if let Err(e) = risk.pre_check(&TradeSignal::Stock(s.clone()), est_price, &st) {
                                error!("risk rejected: {:#}", e);
                                continue;
                            }
                        }

                        if cfg.exec.dry_run {
                            info!("[DRY-RUN] STOCK {:?} {} @ {:?}", s.action, symbol, s.limit_price.unwrap_or(est_price));
                            continue;
                        }

                        let side = match s.action { Action::BTO => OrderAction::Buy, Action::STC => OrderAction::Sell };
                        let qty = s.quantity as f64;

                        // Choose mode & compute effective limit price if needed
                        let is_market = match s.action { Action::BTO => buy_is_market, Action::STC => sell_is_market };
                        let mut limit_px = s.limit_price;
                        if !is_market {
                            if limit_px.is_none() { limit_px = Some(est_price); }
                            let slip = if s.action == Action::BTO { cfg.exec.buy_limit_slippage_pct } else { cfg.exec.sell_limit_slippage_pct };
                            let adj = if s.action == Action::BTO { 1.0 + slip } else { 1.0 - slip };
                            limit_px = limit_px.map(|p| p * adj);
                            est_price = limit_px.unwrap_or(est_price);
                        }

                        // Place
                        let order_id = if is_market {
                            wb.place_stock_market(&symbol, qty, side, &tif).await
                        } else {
                            wb.place_stock_limit(&symbol, qty, side, limit_px.unwrap(), &tif).await
                        };

                        let Ok(order_id) = order_id else { error!("place stock order failed: {:#}", order_id.unwrap_err()); continue; };
                        info!("Placed STOCK order id={}", order_id);

                        // ---- spawn monitor task (NON-blocking) ----
                        let wb_c = Arc::clone(&wb);
                        let state_c = Arc::clone(&state);
                        let tif_c = tif.clone();
                        let cfg_c = cfg.clone();
                        let path_c = cfg.state.path.clone();
                        let symbol_c = symbol.clone();
                        let order_id_c = order_id.clone();
                        tokio::task::spawn_local(async move {
                            if s.action == Action::BTO {
                                monitor_buy_stock_and_update(wb_c, state_c, &cfg_c, &path_c, symbol_c, qty, order_id_c).await;
                            } else {
                                monitor_sell_stock_and_update(wb_c, state_c, &cfg_c, &path_c, symbol, qty, is_market, limit_px, tif_c, order_id).await;
                            }
                        });
                    }

                    TradeSignal::Option(o) => {
                        let symbol = sanitize_symbol(&o.symbol);
                        let contract = match wb.find_option_contract(&symbol, o.strike, o.call_put, &o.expiry_mmdd).await {
                            Ok(c) => c,
                            Err(e) => { error!("find option contract failed: {:#}", e); continue; }
                        };

                        // Base price for risk & possible derived limit when needed
                        let mut est_price = if let (OrderType::Limit, Some(p)) = (o.order_type, o.limit_price) {
                            p
                        } else {
                            wb.mid_price(contract.ticker_id).await.unwrap_or(0.0)
                        };

                        {
                            let st = state.lock().await;
                            if let Err(e) = risk.pre_check(&TradeSignal::Option(o.clone()), est_price, &st) {
                                error!("risk rejected: {:#}", e);
                                continue;
                            }
                        }

                        if cfg.exec.dry_run {
                            info!("[DRY-RUN] OPTION {:?} {} {}{} {} @ {:?}", o.action, symbol, o.strike, o.call_put, o.expiry_mmdd, o.limit_price.unwrap_or(est_price));
                            continue;
                        }

                        let side = match o.action { Action::BTO => OrderAction::Buy, Action::STC => OrderAction::Sell };
                        let qty = o.quantity as f64;

                        // Choose mode & compute effective limit price if needed
                        let is_market = match o.action { Action::BTO => buy_is_market, Action::STC => sell_is_market };
                        let mut limit_px = o.limit_price;
                        if !is_market {
                            if limit_px.is_none() { limit_px = Some(est_price); }
                            let slip = if o.action == Action::BTO { cfg.exec.buy_limit_slippage_pct } else { cfg.exec.sell_limit_slippage_pct };
                            let adj = if o.action == Action::BTO { 1.0 + slip } else { 1.0 - slip };
                            limit_px = limit_px.map(|p| p * adj);
                            est_price = limit_px.unwrap_or(est_price);
                        }

                        // Place
                        let order_id = if is_market {
                            wb.place_option_market(&contract, qty, side, &tif).await
                        } else {
                            wb.place_option_limit(&contract, qty, side, limit_px.unwrap(), &tif).await
                        };

                        let Ok(order_id) = order_id else { error!("place option order failed: {:#}", order_id.unwrap_err()); continue; };
                        info!("Placed OPTION order id={}", order_id);

                        // ---- spawn monitor task (NON-blocking) ----
                        let wb_c = Arc::clone(&wb);
                        let state_c = Arc::clone(&state);
                        let tif_c = tif.clone();
                        let cfg_c = cfg.clone();
                        let path_c = cfg.state.path.clone();
                        let order_id_c = order_id.clone();
                        let symbol_c = symbol.clone();
                        tokio::task::spawn_local(async move {
                            if o.action == Action::BTO {
                                monitor_buy_option_and_update(wb_c, state_c, &cfg_c, &path_c, symbol_c, o.strike, o.call_put, o.expiry_mmdd.clone(), qty as u32, order_id_c).await;
                            } else {
                                monitor_sell_option_and_update(wb_c, state_c, &cfg_c, &path_c, symbol, o.strike, o.call_put, &o.expiry_mmdd, qty as u32, is_market, limit_px, tif_c, order_id, contract.ticker_id).await;
                            }
                        });
                    }
                }
            }

            _ = sync_ticker.tick() => {
                match wb.positions_simple().await {
                    Ok(holdings) => {
                        let mut st = state.lock().await;
                        st.set_holdings(holdings);
                        if let Err(e) = st.save(&cfg.state.path) { error!("state save failed: {:#}", e); }
                        else { info!("Holdings synced from Webull"); }
                    }
                    Err(e) => error!("Periodic holdings sync failed: {:#}", e),
                }
            }
        }
    }

    let _ = discord_handle.await;
    Ok(())
}

// ---------------- Helpers: monitoring & state updates ----------------

async fn poll_until_filled(
    wb: Arc<webull_client::WbCtx>,
    order_id: &str,
    max_sec: u64,
) -> anyhow::Result<OrderInfo> {
    let start = std::time::Instant::now();
    loop {
        let info = wb.get_order_info(order_id).await?;
        match info.status {
            OrderStatus::Filled => return Ok(info.clone()),
            OrderStatus::PartiallyFilled | OrderStatus::Working | OrderStatus::Unknown(_) => {}
            OrderStatus::Canceled | OrderStatus::Rejected => return Ok(info),
        }
        if start.elapsed() >= Duration::from_secs(max_sec) {
            return Ok(info);
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
}

async fn monitor_buy_stock_and_update(
    wb: Arc<webull_client::WbCtx>,
    state: Arc<Mutex<state::BotState>>,
    cfg: &config::AppConfig,
    state_path: &str,
    symbol: String,
    qty: f64,
    order_id: String,
) {
    let info = match poll_until_filled(Arc::clone(&wb), &order_id, cfg.exec.buy_timeout_sec).await {
        Ok(i) => i,
        Err(e) => {
            error!("poll buy stock failed: {:#}", e);
            return;
        }
    };
    match info.status {
        OrderStatus::Filled => {
            let mut st = state.lock().await;
            st.upsert_stock_buy_with_cost(&symbol, qty, info.avg_fill_price);
            let _ = st.save(state_path);
        }
        OrderStatus::PartiallyFilled => {
            let q = info.filled_qty;
            if q > 0.0 {
                let mut st = state.lock().await;
                st.upsert_stock_buy_with_cost(&symbol, q, info.avg_fill_price);
                let _ = st.save(state_path);
            }
            let _ = wb.cancel_order(&order_id).await;
        }
        OrderStatus::Working | OrderStatus::Unknown(_) => {
            let _ = wb.cancel_order(&order_id).await;
            info!("BUY stock timeout -> canceled pending order");
        }
        _ => {}
    }
}

async fn monitor_sell_stock_and_update(
    wb: Arc<webull_client::WbCtx>,
    state: Arc<Mutex<state::BotState>>,
    cfg: &config::AppConfig,
    state_path: &str,
    symbol: String,
    orig_qty: f64,
    was_market: bool,
    _limit_px: Option<f64>,
    tif: TimeInForce,
    order_id: String,
) {
    let date = Local::now().date_naive();
    let info = match poll_until_filled(Arc::clone(&wb), &order_id, cfg.exec.sell_timeout_sec).await
    {
        Ok(i) => i,
        Err(e) => {
            error!("poll sell stock failed: {:#}", e);
            return;
        }
    };
    match info.status {
        OrderStatus::Filled => {
            let mut st = state.lock().await;
            let _ = st.realize_stock_sell(&symbol, orig_qty, info.avg_fill_price, date);
            let _ = st.save(state_path);
        }
        OrderStatus::PartiallyFilled | OrderStatus::Working | OrderStatus::Unknown(_) => {
            let filled = info.filled_qty;
            if filled > 0.0 {
                let mut st = state.lock().await;
                let _ = st.realize_stock_sell(&symbol, filled, info.avg_fill_price, date);
                let _ = st.save(state_path);
            }
            if !was_market {
                let _ = wb.cancel_order(&order_id).await;
                let remaining = (orig_qty - filled).max(0.0);
                if remaining > 0.0 {
                    match wb
                        .place_stock_market(&symbol, remaining, OrderAction::Sell, &tif)
                        .await
                    {
                        Ok(mid) => {
                            info!(
                                "SELL stock timeout -> converted remaining to MARKET (new id={})",
                                mid
                            );
                            if let Ok(i2) =
                                poll_until_filled(Arc::clone(&wb), &mid, cfg.exec.sell_timeout_sec)
                                    .await
                            {
                                if i2.filled_qty > 0.0 {
                                    let mut st = state.lock().await;
                                    let _ = st.realize_stock_sell(
                                        &symbol,
                                        i2.filled_qty,
                                        i2.avg_fill_price,
                                        date,
                                    );
                                    let _ = st.save(state_path);
                                }
                            }
                        }
                        Err(e) => error!("convert sell to market failed: {:#}", e),
                    }
                }
            }
        }
        OrderStatus::Canceled | OrderStatus::Rejected => {}
    }
}

async fn monitor_buy_option_and_update(
    wb: Arc<webull_client::WbCtx>,
    state: Arc<Mutex<state::BotState>>,
    cfg: &config::AppConfig,
    state_path: &str,
    symbol: String,
    strike: f64,
    cp: char,
    expiry: String,
    qty: u32,
    order_id: String,
) {
    let info = match poll_until_filled(Arc::clone(&wb), &order_id, cfg.exec.buy_timeout_sec).await {
        Ok(i) => i,
        Err(e) => {
            error!("poll buy option failed: {:#}", e);
            return;
        }
    };
    match info.status {
        OrderStatus::Filled => {
            let mut st = state.lock().await;
            st.upsert_option_buy_with_cost(&symbol, strike, cp, &expiry, qty, info.avg_fill_price);
            let _ = st.save(state_path);
        }
        OrderStatus::PartiallyFilled => {
            let q = info.filled_qty as u32;
            if q > 0 {
                let mut st = state.lock().await;
                st.upsert_option_buy_with_cost(
                    &symbol,
                    strike,
                    cp,
                    &expiry,
                    q,
                    info.avg_fill_price,
                );
                let _ = st.save(state_path);
            }
            let _ = wb.cancel_order(&order_id).await;
        }
        OrderStatus::Working | OrderStatus::Unknown(_) => {
            let _ = wb.cancel_order(&order_id).await;
            info!("BUY option timeout -> canceled pending order");
        }
        _ => {}
    }
}

async fn monitor_sell_option_and_update(
    wb: Arc<webull_client::WbCtx>,
    state: Arc<Mutex<state::BotState>>,
    cfg: &config::AppConfig,
    state_path: &str,
    symbol: String,
    strike: f64,
    cp: char,
    expiry: &str,
    orig_qty: u32,
    was_market: bool,
    _limit_px: Option<f64>,
    tif: TimeInForce,
    order_id: String,
    _ticker_id: i64,
) {
    let date = Local::now().date_naive();
    let info = match poll_until_filled(Arc::clone(&wb), &order_id, cfg.exec.sell_timeout_sec).await
    {
        Ok(i) => i,
        Err(e) => {
            error!("poll sell option failed: {:#}", e);
            return;
        }
    };
    match info.status {
        OrderStatus::Filled => {
            let mut st = state.lock().await;
            let _ = st.realize_option_sell(
                &symbol,
                strike,
                cp,
                expiry,
                orig_qty,
                info.avg_fill_price,
                date,
            );
            let _ = st.save(state_path);
        }
        OrderStatus::PartiallyFilled | OrderStatus::Working | OrderStatus::Unknown(_) => {
            let filled = info.filled_qty as u32;
            if filled > 0 {
                let mut st = state.lock().await;
                let _ = st.realize_option_sell(
                    &symbol,
                    strike,
                    cp,
                    expiry,
                    filled,
                    info.avg_fill_price,
                    date,
                );
                let _ = st.save(state_path);
            }
            if !was_market {
                let _ = wb.cancel_order(&order_id).await;
                let remaining = orig_qty.saturating_sub(filled);
                if remaining > 0 {
                    match wb
                        .place_option_market(
                            &wb.find_option_contract(&symbol, strike, cp, expiry)
                                .await
                                .unwrap(),
                            remaining as f64,
                            OrderAction::Sell,
                            &tif,
                        )
                        .await
                    {
                        Ok(mid) => {
                            info!(
                                "SELL option timeout -> converted remaining to MARKET (new id={})",
                                mid
                            );
                            if let Ok(i2) =
                                poll_until_filled(Arc::clone(&wb), &mid, cfg.exec.sell_timeout_sec)
                                    .await
                            {
                                if i2.filled_qty > 0.0 {
                                    let mut st = state.lock().await;
                                    let _ = st.realize_option_sell(
                                        &symbol,
                                        strike,
                                        cp,
                                        expiry,
                                        i2.filled_qty as u32,
                                        i2.avg_fill_price,
                                        date,
                                    );
                                    let _ = st.save(state_path);
                                }
                            }
                        }
                        Err(e) => error!("convert sell option to market failed: {:#}", e),
                    }
                }
            }
        }
        OrderStatus::Canceled | OrderStatus::Rejected => {}
    }
}
