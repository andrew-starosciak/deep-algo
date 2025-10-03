use anyhow::{Context, Result};
use crate::database::OhlcvRecord;
use std::fs::File;
use csv::Writer;

pub struct CsvStorage;

impl CsvStorage {
    /// Writes OHLCV records to CSV file compatible with HistoricalDataProvider
    ///
    /// Format: timestamp,symbol,open,high,low,close,volume
    ///
    /// # Errors
    /// Returns error if file cannot be created or writing fails
    pub fn write_ohlcv(path: &str, records: &[OhlcvRecord]) -> Result<()> {
        let file = File::create(path)
            .with_context(|| format!("Failed to create CSV file: {}", path))?;
        let mut writer = Writer::from_writer(file);

        // Write header
        writer.write_record(["timestamp", "symbol", "open", "high", "low", "close", "volume"])?;

        // Sort records by timestamp (ascending) to match backtest expectations
        let mut sorted = records.to_vec();
        sorted.sort_by_key(|r| r.timestamp);

        // Write data rows
        for record in sorted {
            writer.write_record(&[
                record.timestamp.to_rfc3339(),  // ISO 8601 format
                record.symbol.clone(),
                record.open.to_string(),
                record.high.to_string(),
                record.low.to_string(),
                record.close.to_string(),
                record.volume.to_string(),
            ])?;
        }

        writer.flush()?;
        Ok(())
    }
}
