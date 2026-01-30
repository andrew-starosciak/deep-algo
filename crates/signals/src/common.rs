use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::US::Eastern;
use tokio_tungstenite::connect_async;
use url::Url;

pub const BINANCE_FUTURES_WS: &str = "wss://fstream.binance.com/ws/";
pub const BINANCE_LIQUIDATION_WS: &str = "wss://fstream.binance.com/ws/!forceOrder@arr";

/// Format a number with thousands separators
pub fn format_usd(value: f64, decimals: usize) -> String {
    let formatted = match decimals {
        0 => format!("{:.0}", value),
        2 => format!("{:.2}", value),
        _ => format!("{:.2}", value),
    };

    let parts: Vec<&str> = formatted.split('.').collect();
    let int_part = parts[0];
    let dec_part = parts.get(1);

    let int_with_commas: String = int_part
        .chars()
        .rev()
        .enumerate()
        .flat_map(|(i, c)| {
            if i > 0 && i % 3 == 0 {
                vec![',', c]
            } else {
                vec![c]
            }
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    match dec_part {
        Some(d) if decimals > 0 => format!("{}.{}", int_with_commas, d),
        _ => int_with_commas,
    }
}

pub async fn connect_websocket(
    url: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let url = Url::parse(url)?;
    let (ws_stream, _) = connect_async(url).await?;
    Ok(ws_stream)
}

pub fn format_time_eastern(timestamp_ms: i64) -> String {
    let dt = Utc.timestamp_millis_opt(timestamp_ms).unwrap();
    let eastern_time: DateTime<chrono_tz::Tz> = dt.with_timezone(&Eastern);
    eastern_time.format("%H:%M:%S").to_string()
}

pub fn format_time_utc(timestamp_ms: i64) -> String {
    let dt = Utc.timestamp_millis_opt(timestamp_ms).unwrap();
    dt.format("%H:%M:%S").to_string()
}

pub fn strip_usdt(symbol: &str) -> String {
    symbol.replace("USDT", "").replace("usdt", "")
}

pub fn truncate_symbol(symbol: &str, max_len: usize) -> String {
    let stripped = strip_usdt(symbol);
    if stripped.len() > max_len {
        stripped[..max_len].to_string()
    } else {
        stripped
    }
}
