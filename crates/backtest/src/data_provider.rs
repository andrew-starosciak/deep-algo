use algo_trade_core::events::MarketEvent;
use algo_trade_core::traits::DataProvider;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::str::FromStr;

pub struct HistoricalDataProvider {
    events: Vec<MarketEvent>,
    current_index: usize,
}

impl HistoricalDataProvider {
    /// Creates a new `HistoricalDataProvider` from a CSV file.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The CSV file cannot be opened
    /// - The CSV file has invalid format
    /// - Timestamp parsing fails
    /// - Decimal parsing fails for OHLCV values
    pub fn from_csv(path: &str) -> Result<Self> {
        let mut reader = csv::Reader::from_path(path)?;
        let mut events = Vec::new();

        for result in reader.records() {
            let record = result?;
            // Assuming CSV format: timestamp,symbol,open,high,low,close,volume
            let timestamp: DateTime<Utc> = record[0].parse()?;
            let symbol = record[1].to_string();
            let open = Decimal::from_str(&record[2])?;
            let high = Decimal::from_str(&record[3])?;
            let low = Decimal::from_str(&record[4])?;
            let close = Decimal::from_str(&record[5])?;
            let volume = Decimal::from_str(&record[6])?;

            events.push(MarketEvent::Bar {
                symbol,
                open,
                high,
                low,
                close,
                volume,
                timestamp,
            });
        }

        // Sort by timestamp to ensure chronological order
        events.sort_by_key(|e| match e {
            MarketEvent::Bar { timestamp, .. }
            | MarketEvent::Trade { timestamp, .. }
            | MarketEvent::Quote { timestamp, .. } => *timestamp,
        });

        Ok(Self {
            events,
            current_index: 0,
        })
    }
}

#[async_trait]
impl DataProvider for HistoricalDataProvider {
    async fn next_event(&mut self) -> Result<Option<MarketEvent>> {
        if self.current_index < self.events.len() {
            let event = self.events[self.current_index].clone();
            self.current_index += 1;
            Ok(Some(event))
        } else {
            Ok(None)
        }
    }
}
