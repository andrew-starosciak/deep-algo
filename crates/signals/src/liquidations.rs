use crate::common::{
    connect_websocket, format_time_eastern, format_usd, truncate_symbol, BINANCE_LIQUIDATION_WS,
};
use colored::Colorize;
use futures_util::StreamExt;
use serde::Deserialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct LiquidationEvent {
    #[serde(rename = "o")]
    pub order: LiquidationOrder,
}

#[derive(Debug, Deserialize)]
pub struct LiquidationOrder {
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "S")]
    pub side: String,
    #[serde(rename = "o")]
    pub order_type: String,
    #[serde(rename = "f")]
    pub time_in_force: String,
    #[serde(rename = "q")]
    pub original_quantity: String,
    #[serde(rename = "p")]
    pub price: String,
    #[serde(rename = "ap")]
    pub average_price: String,
    #[serde(rename = "X")]
    pub order_status: String,
    #[serde(rename = "l")]
    pub last_filled_quantity: String,
    #[serde(rename = "z")]
    pub filled_accumulated_quantity: String,
    #[serde(rename = "T")]
    pub trade_time: i64,
}

pub struct LiquidationMonitor {
    filename: String,
    min_usd_size: f64,
}

impl LiquidationMonitor {
    pub fn new(filename: &str, min_usd_size: f64) -> Self {
        Self {
            filename: filename.to_string(),
            min_usd_size,
        }
    }

    pub fn default_all() -> Self {
        Self::new("binance.csv", 3000.0)
    }

    pub fn default_big() -> Self {
        Self::new("binance_bigliqs.csv", 100000.0)
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
                "symbol,side,order_type,time_in_force,original_quantity,price,average_price,order_status,order_last_filled_quantity,order_filled_accumulated_quantity,order_trade_time,usd_size"
            )?;
        }
        Ok(())
    }

    fn write_to_csv(&self, order: &LiquidationOrder, usd_size: f64) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.filename)?;

        let symbol = order.symbol.replace("USDT", "");
        writeln!(
            file,
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            symbol,
            order.side,
            order.order_type,
            order.time_in_force,
            order.original_quantity,
            order.price,
            order.average_price,
            order.order_status,
            order.last_filled_quantity,
            order.filled_accumulated_quantity,
            order.trade_time,
            usd_size
        )?;
        Ok(())
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_csv_exists()?;

        loop {
            if let Err(e) = self.stream_liquidations().await {
                eprintln!("Error in liquidation stream: {}", e);
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    }

    async fn stream_liquidations(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut ws_stream = connect_websocket(BINANCE_LIQUIDATION_WS).await?;

        while let Some(msg) = ws_stream.next().await {
            let msg = msg?;
            if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
                let event: LiquidationEvent = serde_json::from_str(&text)?;
                let order = &event.order;

                let filled_quantity: f64 = order.filled_accumulated_quantity.parse()?;
                let price: f64 = order.price.parse()?;
                let usd_size = filled_quantity * price;
                let time_est = format_time_eastern(order.trade_time);
                let symbol = truncate_symbol(&order.symbol, 4);

                if usd_size > self.min_usd_size {
                    self.print_liquidation(&symbol, &order.side, &time_est, usd_size);
                }

                let _ = self.write_to_csv(order, usd_size);
            }
        }

        Ok(())
    }

    fn print_liquidation(&self, symbol: &str, side: &str, time: &str, usd_size: f64) {
        let liquidation_type = if side == "SELL" { "L LIQ" } else { "S LIQ" };

        if self.min_usd_size >= 100000.0 {
            // Big liquidations mode (blue/magenta)
            let output = format!(
                "{} {} {} {}",
                liquidation_type,
                symbol,
                time,
                format_usd(usd_size, 2)
            );
            let colored_output = if side == "SELL" {
                output.white().on_blue().bold()
            } else {
                output.white().on_magenta().bold()
            };
            println!("{}\n", colored_output);
        } else {
            // All liquidations mode (green/red with stars for big ones)
            let base_output = format!(
                "{} {} {} {}",
                liquidation_type,
                symbol,
                time,
                format_usd(usd_size, 0)
            );

            if usd_size > 250000.0 {
                let output = format!("***{}", base_output);
                let colored = if side == "SELL" {
                    output.white().on_green().bold()
                } else {
                    output.white().on_red().bold()
                };
                for _ in 0..4 {
                    println!("{}", colored);
                }
            } else if usd_size > 100000.0 {
                let output = format!("*{}", base_output);
                let colored = if side == "SELL" {
                    output.white().on_green().bold()
                } else {
                    output.white().on_red().bold()
                };
                for _ in 0..2 {
                    println!("{}", colored);
                }
            } else if usd_size > 25000.0 {
                let colored = if side == "SELL" {
                    base_output.white().on_green().bold()
                } else {
                    base_output.white().on_red().bold()
                };
                println!("{}", colored);
            } else {
                let colored = if side == "SELL" {
                    base_output.white().on_green()
                } else {
                    base_output.white().on_red()
                };
                println!("{}", colored);
            }
            println!();
        }
    }
}
