//! Small helpers.

pub fn tif_from_str(s: &str) -> webull_unofficial::models::TimeInForce {
    match s.to_ascii_uppercase().as_str() {
        "GTC" => webull_unofficial::models::TimeInForce::GoodTillCancel,
        _ => webull_unofficial::models::TimeInForce::Day,
    }
}

pub fn sanitize_symbol(sym: &str) -> String {
    sym.trim().to_uppercase()
}

pub fn mmdd_digits(s: &str) -> Option<String> {
    // Convert "08/16" or "08-16" to "0816"
    let d: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.len() == 4 {
        Some(d)
    } else {
        None
    }
}

pub fn last4_digits(s: &str) -> Option<String> {
    // Extract last 4 digits of a date-like string, e.g., "2025-08-16" -> "0816"
    let d: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.len() >= 4 {
        Some(d[d.len() - 4..].to_string())
    } else {
        None
    }
}
