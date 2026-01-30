use anyhow::Result;
use arrow::array::{ArrayRef, Decimal128Array, StringArray, TimestampMillisecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::sync::Arc;

pub struct ParquetStorage;

impl ParquetStorage {
    /// Writes OHLCV records to a Parquet file.
    ///
    /// # Errors
    /// Returns an error if the file cannot be created or if writing to the Parquet file fails.
    pub fn write_ohlcv_batch(path: &str, records: &[OhlcvRecord]) -> Result<()> {
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "timestamp",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                false,
            ),
            Field::new("symbol", DataType::Utf8, false),
            Field::new("exchange", DataType::Utf8, false),
            Field::new("open", DataType::Decimal128(20, 8), false),
            Field::new("high", DataType::Decimal128(20, 8), false),
            Field::new("low", DataType::Decimal128(20, 8), false),
            Field::new("close", DataType::Decimal128(20, 8), false),
            Field::new("volume", DataType::Decimal128(20, 8), false),
        ]));

        let timestamps: Vec<i64> = records
            .iter()
            .map(|r| r.timestamp.timestamp_millis())
            .collect();
        let symbols: Vec<String> = records.iter().map(|r| r.symbol.clone()).collect();
        let exchanges: Vec<String> = records.iter().map(|r| r.exchange.clone()).collect();

        let timestamp_array = TimestampMillisecondArray::from(timestamps);
        let symbol_array = StringArray::from(symbols);
        let exchange_array = StringArray::from(exchanges);

        // Convert Decimal to i128 for Decimal128Array
        let open_array = Decimal128Array::from(
            records
                .iter()
                .map(|r| Some(r.open.mantissa()))
                .collect::<Vec<_>>(),
        )
        .with_precision_and_scale(20, 8)?;

        let high_array = Decimal128Array::from(
            records
                .iter()
                .map(|r| Some(r.high.mantissa()))
                .collect::<Vec<_>>(),
        )
        .with_precision_and_scale(20, 8)?;

        let low_array = Decimal128Array::from(
            records
                .iter()
                .map(|r| Some(r.low.mantissa()))
                .collect::<Vec<_>>(),
        )
        .with_precision_and_scale(20, 8)?;

        let close_array = Decimal128Array::from(
            records
                .iter()
                .map(|r| Some(r.close.mantissa()))
                .collect::<Vec<_>>(),
        )
        .with_precision_and_scale(20, 8)?;

        let volume_array = Decimal128Array::from(
            records
                .iter()
                .map(|r| Some(r.volume.mantissa()))
                .collect::<Vec<_>>(),
        )
        .with_precision_and_scale(20, 8)?;

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(timestamp_array) as ArrayRef,
                Arc::new(symbol_array) as ArrayRef,
                Arc::new(exchange_array) as ArrayRef,
                Arc::new(open_array) as ArrayRef,
                Arc::new(high_array) as ArrayRef,
                Arc::new(low_array) as ArrayRef,
                Arc::new(close_array) as ArrayRef,
                Arc::new(volume_array) as ArrayRef,
            ],
        )?;

        let file = File::create(path)?;
        let props = WriterProperties::builder()
            .set_compression(parquet::basic::Compression::SNAPPY)
            .build();
        let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;

        writer.write(&batch)?;
        writer.close()?;

        Ok(())
    }
}

use crate::database::OhlcvRecord;
