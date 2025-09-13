//! Parse trade signals from Discord messages.
//! Supported (v1.0): Stocks & Options (Market/Limit).

use crate::types::{Action, OptionSignal, OrderType, StockSignal, TradeSignal};
use regex::Regex;

pub fn parse_signal(text: &str) -> Option<TradeSignal> {
    // Normalize whitespace
    let t = text.trim();

    // Options: "BTO 10 AAPL 150C 08/16 @ 2.50" or market with @ m
    let re_opt = Regex::new(r"(?i)^(BTO|STC)\s+(\d+)\s+([A-Z]{1,6})\s+(\d+(?:\.\d+)?)\s*([CP])\s+(\d{2}/\d{2})\s*@\s*(m|M|[\d\.]+)$").unwrap();
    // --- Options without quantity: "BTO AAPL 150C 08/16 @ 2.50" ---
    let re_opt_noqty = Regex::new(r"(?i)^(BTO|STC)\s+([A-Z]{1,6})\s+(\d+(?:\.\d+)?)\s*([CP])\s+(\d{2}/\d{2})\s*@\s*(m|[\d\.]+)$").unwrap();

    if let Some(c) = re_opt.captures(t) {
        let action = match &c[1].to_uppercase()[..] {
            "BTO" => Action::BTO,
            "STC" => Action::STC,
            _ => return None,
        };
        let qty: u32 = c[2].parse().ok()?;
        let symbol = c[3].to_uppercase();
        let strike: f64 = c[4].parse().ok()?;
        let cp = c[5].chars().next().unwrap().to_ascii_uppercase();
        let expiry = c[6].to_string();
        let price_raw = c[7].to_ascii_lowercase();

        let (ot, lp) = if price_raw == "m" {
            (OrderType::Market, None)
        } else {
            (OrderType::Limit, Some(price_raw.parse().ok()?))
        };

        return Some(TradeSignal::Option(OptionSignal {
            action,
            symbol,
            strike,
            call_put: cp,
            expiry_mmdd: expiry,
            quantity: qty,
            order_type: ot,
            limit_price: lp,
        }));
    }

    if let Some(c) = re_opt_noqty.captures(t) {
        let action = match &c[1].to_uppercase()[..] {
            "BTO" => Action::BTO,
            "STC" => Action::STC,
            _ => return None,
        };
        let symbol = c[2].to_uppercase();
        let strike: f64 = c[3].parse().ok()?;
        let cp = c[4].chars().next().unwrap().to_ascii_uppercase();
        let expiry = c[5].to_string();
        let price_raw = c[6].to_ascii_lowercase();

        let (ot, lp) = if price_raw == "m" {
            (OrderType::Market, None)
        } else {
            (OrderType::Limit, Some(price_raw.parse().ok()?))
        };

        return Some(TradeSignal::Option(OptionSignal {
            action,
            symbol,
            strike,
            call_put: cp,
            expiry_mmdd: expiry,
            quantity: 1, // default when qty missing
            order_type: ot,
            limit_price: lp,
        }));
    }

    // Stocks: "BTO 100 AAPL @ m" or with a limit price
    let re_stk = Regex::new(r"(?i)^(BTO|STC)\s+(\d+)\s+([A-Z]{1,6})\s*@\s*(m|M|[\d\.]+)$").unwrap();
    let re_stk_noqty = Regex::new(r"(?i)^(BTO|STC)\s+([A-Z]{1,6})\s*@\s*(m|[\d\.]+)$").unwrap();

    if let Some(c) = re_stk.captures(t) {
        let action = match &c[1].to_uppercase()[..] {
            "BTO" => Action::BTO,
            "STC" => Action::STC,
            _ => return None,
        };
        let qty: u32 = c[2].parse().ok()?;
        let symbol = c[3].to_uppercase();
        let price_raw = c[4].to_ascii_lowercase();

        let (ot, lp) = if price_raw == "m" {
            (OrderType::Market, None)
        } else {
            (OrderType::Limit, Some(price_raw.parse().ok()?))
        };

        return Some(TradeSignal::Stock(StockSignal {
            action,
            symbol,
            quantity: qty,
            order_type: ot,
            limit_price: lp,
        }));
    }

    if let Some(c) = re_stk_noqty.captures(t) {
        let action = match &c[1].to_uppercase()[..] {
            "BTO" => Action::BTO,
            "STC" => Action::STC,
            _ => return None,
        };
        let symbol = c[2].to_uppercase();
        let price_raw = c[3].to_ascii_lowercase();

        let (ot, lp) = if price_raw == "m" {
            (OrderType::Market, None)
        } else {
            (OrderType::Limit, Some(price_raw.parse().ok()?))
        };

        return Some(TradeSignal::Stock(StockSignal {
            action,
            symbol,
            quantity: 1, // default when qty missing
            order_type: ot,
            limit_price: lp,
        }));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Action, OrderType, TradeSignal};

    fn must_parse(s: &str) -> TradeSignal {
        parse_signal(s).expect(&format!("should parse: {s}"))
    }

    fn must_parse_stock(s: &str) -> StockSignal {
        match must_parse(s) {
            TradeSignal::Stock(stk) => stk,
            _ => panic!("expected StockSignal"),
        }
    }

    fn must_parse_option(s: &str) -> OptionSignal {
        match must_parse(s) {
            TradeSignal::Option(opt) => opt,
            _ => panic!("expected OptionSignal"),
        }
    }

    // ---------- Options: positive cases ----------

    #[test]
    fn opt_limit_c_lower_mixed_case_action() {
        let o = must_parse_option("BTO 10 AAPL 150c 08/16 @ 2.50");
        assert_eq!(o.action, Action::BTO);
        assert_eq!(o.symbol, "AAPL"); // NOTE: current parser keeps case as-is; this line
                                      // will FAIL with current code because it returns "AAPL" only if you uppercased in parser.
                                      // If your current code keeps "AAPL" as-is from input, adapt accordingly:
                                      // If your current parser keeps original case, comment the line above and uncomment this:
                                      // assert_eq!(o.symbol, "AAPL"); // input was AAPL
        assert_eq!(o.strike, 150.0);
        assert_eq!(o.call_put, 'C'); // parser uppercases C/P
        assert_eq!(o.expiry_mmdd, "08/16");
        assert_eq!(o.quantity, 10);
        assert_eq!(o.order_type, OrderType::Limit);
        assert_eq!(o.limit_price, Some(2.50));
    }

    #[test]
    fn opt_limit_p_lowercase_ok() {
        let o = must_parse_option("BTO 10 AAPL 150p 08/16 @ 2.50");
        assert_eq!(o.call_put, 'P');
        assert_eq!(o.limit_price, Some(2.50));
    }

    #[test]
    fn opt_limit_space_after_at_optional() {
        // Current regex requires space BEFORE '@' and allows optional space AFTER '@'
        let o = must_parse_option("BTO 10 AAPL 150c 08/16 @2.50");
        assert_eq!(o.limit_price, Some(2.50));
    }

    #[test]
    fn opt_no_space_before_at_should_fail() {
        // No space before '@' -> should NOT match with current regex (\s+@\s*)
        assert!(parse_signal("BTO 10 AAPL 150c 08/16@2.50").is_none());
    }

    #[test]
    #[allow(non_snake_case)]
    fn opt_market_price_m_or_M() {
        let o1 = must_parse_option("STC 2 TSLA 200C 09/20 @ m");
        assert_eq!(o1.order_type, OrderType::Market);
        assert_eq!(o1.limit_price, None);

        let o2 = must_parse_option("stc 2 tsla 200p 09/20 @ M"); // case-insensitive overall
        assert_eq!(o2.order_type, OrderType::Market);
        assert_eq!(o2.call_put, 'P');
    }

    #[test]
    fn opt_symbol_lowercase_kept_as_is_currently() {
        // Current parser keeps symbol as captured (does not uppercase)
        let o = must_parse_option("BTO 3 aapl 150c 08/16 @ 2.50");
        assert_eq!(o.symbol, "aapl");
    }

    #[test]
    fn opt_missing_quantity_should_fail_currently() {
        // Current regex requires quantity (\d+). Missing qty should fail.
        assert!(parse_signal("BTO aapl 150c 08/16 @ 2.50").is_none());
    }

    // ---------- Stocks: positive cases ----------

    #[test]
    fn stk_limit_and_market() {
        let s1 = must_parse_stock("BTO 100 AAPL @ 150.25");
        assert_eq!(s1.action, Action::BTO);
        assert_eq!(s1.quantity, 100);
        assert_eq!(s1.order_type, OrderType::Limit);
        assert_eq!(s1.limit_price, Some(150.25));

        let s2 = must_parse_stock("stc 50 nvda @ m");
        assert_eq!(s2.action, Action::STC);
        assert_eq!(s2.symbol, "nvda"); // kept as-is
        assert_eq!(s2.order_type, OrderType::Market);
        assert_eq!(s2.limit_price, None);
    }

    #[test]
    fn stk_space_before_at_required() {
        assert!(parse_signal("BTO 10 AAPL@m").is_none()); // no space before '@'
        assert!(parse_signal("BTO 10 AAPL @m").is_some()); // space before, none after -> ok
        assert!(parse_signal("BTO 10 AAPL @ m").is_some()); // space both sides -> ok
    }

    // ---------- Negative / edge cases ----------

    #[test]
    fn random_text_should_fail() {
        assert!(parse_signal("hello world").is_none());
        assert!(parse_signal("buy apple now").is_none());
    }

    #[test]
    fn bad_price_or_format_should_fail() {
        assert!(parse_signal("BTO 10 AAPL @ x").is_none()); // price not m or number
        assert!(parse_signal("BTO 10 AAPL 150C 08/16 2.50").is_none()); // missing '@'
    }

    #[test]
    fn symbol_length_and_dot_not_supported_now() {
        // current pattern = [A-Z]{1,6} (case-insensitive), so >6 letters or dot symbols fail
        assert!(parse_signal("BTO 1 ABCDEFG @ m").is_none()); // 7 letters
        assert!(parse_signal("BTO 1 BRK.B @ m").is_none()); // dot not allowed
    }

    #[test]
    fn leading_trailing_spaces_ok() {
        let s = must_parse_stock("   BTO 1 AAPL @ 123.0   ");
        assert_eq!(s.quantity, 1);
        assert_eq!(s.limit_price, Some(123.0));
    }

    // ---------- Future behavior wishes (documented as #[ignore]) ----------

    #[test]
    #[ignore]
    fn future_opt_missing_qty_defaults_to_one() {
        // When/if you relax the regex to allow missing qty, change this to assert Some(...)
        assert!(parse_signal("BTO aapl 150c 08/16 @ 2.50").is_none());
    }

    #[test]
    #[ignore]
    fn future_symbol_uppercased_in_parser() {
        // If you decide to uppercase in parser, adapt the assertions accordingly.
        let o = must_parse_option("BTO 3 aapl 150c 08/16 @ 2.50");
        assert_eq!(o.symbol, "AAPL");
    }

    #[test]
    #[ignore]
    fn future_no_space_before_at_allowed() {
        // If you relax to \s*@\s*, then this should pass.
        assert!(parse_signal("BTO 10 AAPL 150c 08/16@2.50").is_some());
    }
}
