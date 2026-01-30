use crate::common::{
    connect_websocket, format_time_eastern, format_usd, strip_usdt, BINANCE_FUTURES_WS,
};
use colored::Colorize;
use futures_util::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Deserialize)]
pub struct AggTradeData {
    #[serde(rename = "E")]
    pub event_time: i64,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "a")]
    pub agg_trade_id: i64,
    #[serde(rename = "p")]
    pub price: String,
    #[serde(rename = "q")]
    pub quantity: String,
    #[serde(rename = "T")]
    pub trade_time: i64,
    #[serde(rename = "m")]
    pub is_buyer_maker: bool,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct TradeKey {
    symbol: String,
    second: String,
    is_buyer_maker: bool,
}

pub struct TradeAggregator {
    buckets: Arc<Mutex<HashMap<TradeKey, f64>>>,
}

impl TradeAggregator {
    pub fn new() -> Self {
        Self {
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn add_trade(&self, symbol: &str, second: &str, usd_size: f64, is_buyer_maker: bool) {
        let key = TradeKey {
            symbol: symbol.to_string(),
            second: second.to_string(),
            is_buyer_maker,
        };
        let mut buckets = self.buckets.lock().await;
        *buckets.entry(key).or_insert(0.0) += usd_size;
    }

    pub async fn check_and_print_trades(&self, min_usd_size: f64) {
        let now = chrono::Utc::now().format("%H:%M:%S").to_string();
        let mut buckets = self.buckets.lock().await;
        let mut to_delete = Vec::new();

        for (key, usd_size) in buckets.iter() {
            if key.second < now && *usd_size > min_usd_size {
                let trade_type = if !key.is_buyer_maker { "BUY" } else { "SELL" };
                let usd_millions = usd_size / 1_000_000.0;

                let output = format!(
                    "{} {} {} ${:.2}m",
                    trade_type, key.symbol, key.second, usd_millions
                );

                let colored = if !key.is_buyer_maker {
                    output.white().on_blue().bold()
                } else {
                    output.white().on_magenta().bold()
                };

                if *usd_size > 3_000_000.0 {
                    // Blink effect (using bright)
                    println!("{}", colored.on_bright_blue());
                } else {
                    println!("{}", colored);
                }

                to_delete.push(key.clone());
            }
        }

        for key in to_delete {
            buckets.remove(&key);
        }
    }
}

impl Default for TradeAggregator {
    fn default() -> Self {
        Self::new()
    }
}

pub struct HugeTradesMonitor {
    symbols: Vec<String>,
    aggregator: TradeAggregator,
    min_usd_size: f64,
}

impl HugeTradesMonitor {
    pub fn new(symbols: Vec<String>, min_usd_size: f64) -> Self {
        Self {
            symbols,
            aggregator: TradeAggregator::new(),
            min_usd_size,
        }
    }

    pub fn default_symbols() -> Vec<String> {
        vec![
            "btcusdt".to_string(),
            "ethusdt".to_string(),
            "solusdt".to_string(),
            "bnbusdt".to_string(),
            "dogeusdt".to_string(),
            "wifusdt".to_string(),
        ]
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut handles = Vec::new();

        for symbol in &self.symbols {
            let symbol = symbol.clone();
            let aggregator = self.aggregator.buckets.clone();

            let handle = tokio::spawn(async move {
                loop {
                    if let Err(e) = Self::stream_trades(&symbol, &aggregator).await {
                        eprintln!("Error in trade stream for {}: {}", symbol, e);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            });
            handles.push(handle);
        }

        let aggregator_buckets = self.aggregator.buckets.clone();
        let min_usd = self.min_usd_size;
        let print_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                let agg = TradeAggregator {
                    buckets: aggregator_buckets.clone(),
                };
                agg.check_and_print_trades(min_usd).await;
            }
        });
        handles.push(print_handle);

        futures_util::future::join_all(handles).await;
        Ok(())
    }

    async fn stream_trades(
        symbol: &str,
        buckets: &Arc<Mutex<HashMap<TradeKey, f64>>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}{}@aggTrade", BINANCE_FUTURES_WS, symbol);
        let mut ws_stream = connect_websocket(&url).await?;

        while let Some(msg) = ws_stream.next().await {
            let msg = msg?;
            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                let data: AggTradeData = serde_json::from_str(&text)?;
                let price: f64 = data.price.parse()?;
                let quantity: f64 = data.quantity.parse()?;
                let usd_size = price * quantity;
                let trade_time = format_time_eastern(data.trade_time);
                let display_symbol = strip_usdt(&data.symbol);

                let key = TradeKey {
                    symbol: display_symbol,
                    second: trade_time,
                    is_buyer_maker: data.is_buyer_maker,
                };

                let mut b = buckets.lock().await;
                *b.entry(key).or_insert(0.0) += usd_size;
            }
        }

        Ok(())
    }
}

pub struct RecentTradesMonitor {
    symbols: Vec<String>,
    filename: String,
    min_usd_size: f64,
}

impl RecentTradesMonitor {
    pub fn new(symbols: Vec<String>, filename: &str, min_usd_size: f64) -> Self {
        Self {
            symbols,
            filename: filename.to_string(),
            min_usd_size,
        }
    }

    pub fn default_symbols() -> Vec<String> {
        vec![
            "btcusdt".to_string(),
            "ethusdt".to_string(),
            "solusdt".to_string(),
            "bnbusdt".to_string(),
            "dogeusdt".to_string(),
            "wifusdt".to_string(),
        ]
    }

    fn ensure_csv_exists(&self) -> std::io::Result<()> {
        if !Path::new(&self.filename).exists() {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&self.filename)?;
            writeln!(
                file,
                "Event Time,Symbol,Aggregate Trade ID,Price,Quantity,First Trade ID,Trade Time,Is Buyer Maker"
            )?;
        }
        Ok(())
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_csv_exists()?;

        let mut handles = Vec::new();

        for symbol in &self.symbols {
            let symbol = symbol.clone();
            let filename = self.filename.clone();
            let min_usd = self.min_usd_size;

            let handle = tokio::spawn(async move {
                loop {
                    if let Err(e) = Self::stream_trades(&symbol, &filename, min_usd).await {
                        eprintln!("Error in trade stream for {}: {}", symbol, e);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            });
            handles.push(handle);
        }

        futures_util::future::join_all(handles).await;
        Ok(())
    }

    async fn stream_trades(
        symbol: &str,
        filename: &str,
        min_usd_size: f64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}{}@aggTrade", BINANCE_FUTURES_WS, symbol);
        let mut ws_stream = connect_websocket(&url).await?;

        while let Some(msg) = ws_stream.next().await {
            let msg = msg?;
            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                let data: AggTradeData = serde_json::from_str(&text)?;
                let price: f64 = data.price.parse()?;
                let quantity: f64 = data.quantity.parse()?;
                let usd_size = price * quantity;
                let trade_time = format_time_eastern(data.trade_time);
                let display_symbol = strip_usdt(&data.symbol);

                if usd_size > min_usd_size {
                    Self::print_trade(&display_symbol, &trade_time, usd_size, data.is_buyer_maker);

                    // Log to CSV
                    if let Ok(mut file) = OpenOptions::new().append(true).open(filename) {
                        let _ = writeln!(
                            file,
                            "{},{},{},{},{},{},{}",
                            data.event_time,
                            data.symbol,
                            data.agg_trade_id,
                            data.price,
                            data.quantity,
                            data.trade_time,
                            data.is_buyer_maker
                        );
                    }
                }
            }
        }

        Ok(())
    }

    fn print_trade(symbol: &str, time: &str, usd_size: f64, is_buyer_maker: bool) {
        let trade_type = if is_buyer_maker { "SELL" } else { "BUY" };

        let (color, stars) = if usd_size >= 500_000.0 {
            (if is_buyer_maker { "magenta" } else { "blue" }, "**")
        } else if usd_size >= 100_000.0 {
            (if is_buyer_maker { "red" } else { "green" }, "*")
        } else {
            (if is_buyer_maker { "red" } else { "green" }, "")
        };

        let output = format!(
            "{} {} {} {} ${}",
            stars,
            trade_type,
            symbol,
            time,
            format_usd(usd_size, 0)
        );

        let is_bold = usd_size >= 50_000.0;

        let colored = match color {
            "magenta" => {
                if is_bold {
                    output.white().on_magenta().bold()
                } else {
                    output.white().on_magenta()
                }
            }
            "blue" => {
                if is_bold {
                    output.white().on_blue().bold()
                } else {
                    output.white().on_blue()
                }
            }
            "red" => {
                if is_bold {
                    output.white().on_red().bold()
                } else {
                    output.white().on_red()
                }
            }
            _ => {
                if is_bold {
                    output.white().on_green().bold()
                } else {
                    output.white().on_green()
                }
            }
        };

        println!("{}", colored);
    }
}
