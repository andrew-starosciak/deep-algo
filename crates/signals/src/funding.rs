use crate::common::{connect_websocket, format_time_utc, strip_usdt, BINANCE_FUTURES_WS};
use colored::Colorize;
use futures_util::StreamExt;
use serde::Deserialize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Deserialize)]
pub struct MarkPriceData {
    #[serde(rename = "E")]
    pub event_time: i64,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "r")]
    pub funding_rate: String,
}

pub struct FundingMonitor {
    symbols: Vec<String>,
    counter: Arc<AtomicUsize>,
    print_lock: Arc<Mutex<()>>,
}

impl FundingMonitor {
    pub fn new(symbols: Vec<String>) -> Self {
        Self {
            symbols,
            counter: Arc::new(AtomicUsize::new(0)),
            print_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn default_symbols() -> Vec<String> {
        vec![
            "btcusdt".to_string(),
            "ethusdt".to_string(),
            "solusdt".to_string(),
            "wifusdt".to_string(),
        ]
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut handles = Vec::new();

        for symbol in &self.symbols {
            let symbol = symbol.clone();
            let counter = Arc::clone(&self.counter);
            let print_lock = Arc::clone(&self.print_lock);
            let num_symbols = self.symbols.len();

            let handle = tokio::spawn(async move {
                loop {
                    if let Err(e) =
                        Self::stream_funding(&symbol, &counter, &print_lock, num_symbols).await
                    {
                        eprintln!("Error in funding stream for {}: {}", symbol, e);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            });
            handles.push(handle);
        }

        futures_util::future::join_all(handles).await;
        Ok(())
    }

    async fn stream_funding(
        symbol: &str,
        counter: &Arc<AtomicUsize>,
        print_lock: &Arc<Mutex<()>>,
        num_symbols: usize,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!("{}{}@markPrice", BINANCE_FUTURES_WS, symbol);
        let mut ws_stream = connect_websocket(&url).await?;

        while let Some(msg) = ws_stream.next().await {
            let msg = msg?;
            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                let _lock = print_lock.lock().await;

                let data: MarkPriceData = serde_json::from_str(&text)?;
                let event_time = format_time_utc(data.event_time);
                let symbol_display = strip_usdt(&data.symbol);
                let funding_rate: f64 = data.funding_rate.parse()?;
                let yearly_funding_rate = funding_rate * 3.0 * 365.0 * 100.0;

                let output = format!("{} funding: {:.2}%", symbol_display, yearly_funding_rate);

                let colored_output = if yearly_funding_rate > 50.0 {
                    output.black().on_red()
                } else if yearly_funding_rate > 30.0 {
                    output.black().on_yellow()
                } else if yearly_funding_rate > 5.0 {
                    output.black().on_cyan()
                } else if yearly_funding_rate < -10.0 {
                    output.black().on_green()
                } else {
                    output.black().on_bright_green()
                };

                println!("{}", colored_output);

                let count = counter.fetch_add(1, Ordering::SeqCst) + 1;
                if count >= num_symbols {
                    println!("{}", format!("{} yrly fund", event_time).white().on_black());
                    counter.store(0, Ordering::SeqCst);
                }
            }
        }

        Ok(())
    }
}
