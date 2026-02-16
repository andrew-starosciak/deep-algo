//! CLOB velocity signal generator.
//!
//! Measures the rate of Polymarket CLOB price change (velocity) rather than
//! just direction. A price that jumps from 0.50 to 0.53 in one tick indicates
//! stronger conviction than a slow drift to 0.535 over 3 minutes.
//!
//! The speed of the initial move correlates with information flow:
//! - **Fast moves** (high velocity): market has strong conviction, signal
//!   confirms the direction with high strength
//! - **Slow drifts** (low velocity): market is uncertain, other signals
//!   can add value before the CLOB accelerates

use algo_trade_core::{Direction, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;

/// Configuration for CLOB velocity signal.
#[derive(Debug, Clone)]
pub struct ClobVelocityConfig {
    /// Lookback window in seconds for velocity calculation.
    /// Shorter = more responsive, longer = smoother.
    pub lookback_secs: f64,
    /// Reference velocity (cents/sec) that maps to strength 1.0.
    /// A move of 3 cents in 10 seconds = 0.3 cents/sec.
    pub reference_velocity: f64,
    /// Minimum displacement from 0.50 to produce a signal.
    /// Filters out noise around the fair-value midpoint.
    pub min_displacement: f64,
    /// Signal weight for composite aggregation.
    pub weight: f64,
}

impl Default for ClobVelocityConfig {
    fn default() -> Self {
        Self {
            lookback_secs: 60.0,
            reference_velocity: 0.2,
            min_displacement: 0.015,
            weight: 1.3,
        }
    }
}

/// CLOB velocity signal generator.
///
/// Reads `ctx.clob_price_history` (timestamped yes-prices from Polymarket)
/// and computes the rate of price change over a configurable lookback window.
pub struct ClobVelocitySignal {
    config: ClobVelocityConfig,
}

impl ClobVelocitySignal {
    /// Creates a new CLOB velocity signal with default config.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ClobVelocityConfig::default(),
        }
    }

    /// Creates a new CLOB velocity signal with custom config.
    #[must_use]
    pub fn with_config(config: ClobVelocityConfig) -> Self {
        Self { config }
    }

    /// Computes velocity from price history.
    ///
    /// Returns (velocity_cents_per_sec, latest_price, displacement_from_midpoint).
    fn compute_velocity(
        &self,
        history: &[(chrono::DateTime<chrono::Utc>, f64)],
    ) -> Option<(f64, f64, f64)> {
        if history.len() < 2 {
            return None;
        }

        let latest = history.last()?;
        let latest_price = latest.1;
        let latest_ts = latest.0;

        // Find the earliest price within our lookback window
        let cutoff = latest_ts
            - chrono::Duration::milliseconds((self.config.lookback_secs * 1000.0) as i64);

        // Get the oldest point within our lookback window
        let earliest_in_window = history
            .iter()
            .find(|(ts, _)| *ts >= cutoff)
            .unwrap_or(&history[0]);

        let dt_secs = (latest_ts - earliest_in_window.0)
            .num_milliseconds() as f64
            / 1000.0;

        if dt_secs < 1.0 {
            return None;
        }

        let dp = latest_price - earliest_in_window.1;
        let velocity = dp / dt_secs; // price units per second
        let displacement = latest_price - 0.5;

        Some((velocity, latest_price, displacement))
    }
}

#[async_trait]
impl SignalGenerator for ClobVelocitySignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        let history = match &ctx.clob_price_history {
            Some(h) if h.len() >= 2 => h,
            _ => return Ok(SignalValue::neutral()),
        };

        let (velocity, _latest_price, displacement) = match self.compute_velocity(history) {
            Some(v) => v,
            None => return Ok(SignalValue::neutral()),
        };

        // Check minimum displacement from midpoint
        if displacement.abs() < self.config.min_displacement {
            return Ok(SignalValue::neutral()
                .with_metadata("velocity", velocity * 100.0)
                .with_metadata("displacement", displacement));
        }

        // Direction from velocity sign
        let direction = if velocity > 0.0 {
            Direction::Up
        } else if velocity < 0.0 {
            Direction::Down
        } else {
            return Ok(SignalValue::neutral());
        };

        // Strength from velocity magnitude (cents/sec normalized to reference)
        let velocity_cents_per_sec = velocity.abs() * 100.0;
        let strength = (velocity_cents_per_sec / self.config.reference_velocity).min(1.0);

        // Confidence scales with how consistent the move is:
        // if velocity and displacement agree, higher confidence
        let velocity_displacement_agree =
            (velocity > 0.0 && displacement > 0.0) || (velocity < 0.0 && displacement < 0.0);
        let confidence = if velocity_displacement_agree {
            (strength * 0.8).min(1.0)
        } else {
            (strength * 0.3).min(1.0)
        };

        Ok(SignalValue::new(direction, strength, confidence)?
            .with_metadata("velocity_cents_per_sec", velocity_cents_per_sec)
            .with_metadata("displacement", displacement)
            .with_metadata("lookback_secs", self.config.lookback_secs))
    }

    fn name(&self) -> &str {
        "clob_velocity"
    }

    fn weight(&self) -> f64 {
        self.config.weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn make_history(
        prices: &[(i64, f64)],
    ) -> Vec<(chrono::DateTime<chrono::Utc>, f64)> {
        let base = Utc.with_ymd_and_hms(2025, 6, 1, 12, 0, 0).unwrap();
        prices
            .iter()
            .map(|(secs_offset, price)| {
                (base + chrono::Duration::seconds(*secs_offset), *price)
            })
            .collect()
    }

    #[tokio::test]
    async fn neutral_when_no_history() {
        let mut signal = ClobVelocitySignal::new();
        let ctx = SignalContext::new(Utc::now(), "BTCUSDT");
        let result = signal.compute(&ctx).await.unwrap();
        assert_eq!(result.direction, Direction::Neutral);
        assert!(result.strength < f64::EPSILON);
    }

    #[tokio::test]
    async fn neutral_when_single_price() {
        let mut signal = ClobVelocitySignal::new();
        let history = make_history(&[(0, 0.50)]);
        let ctx = SignalContext::new(Utc::now(), "BTCUSDT")
            .with_clob_price_history(history);
        let result = signal.compute(&ctx).await.unwrap();
        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn neutral_when_price_at_midpoint() {
        let mut signal = ClobVelocitySignal::new();
        // Price hovers around 0.50 — below min_displacement
        let history = make_history(&[(0, 0.500), (10, 0.501), (20, 0.502)]);
        let ctx = SignalContext::new(Utc::now(), "BTCUSDT")
            .with_clob_price_history(history);
        let result = signal.compute(&ctx).await.unwrap();
        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn up_direction_on_fast_upward_move() {
        let mut signal = ClobVelocitySignal::new();
        // Fast move: 0.50 → 0.55 in 10 seconds = 0.5 cents/sec
        let history = make_history(&[(0, 0.50), (10, 0.55)]);
        let ctx = SignalContext::new(Utc::now(), "BTCUSDT")
            .with_clob_price_history(history);
        let result = signal.compute(&ctx).await.unwrap();
        assert_eq!(result.direction, Direction::Up);
        assert!(result.strength > 0.5);
    }

    #[tokio::test]
    async fn down_direction_on_fast_downward_move() {
        let mut signal = ClobVelocitySignal::new();
        // Fast move: 0.50 → 0.45 in 10 seconds
        let history = make_history(&[(0, 0.50), (10, 0.45)]);
        let ctx = SignalContext::new(Utc::now(), "BTCUSDT")
            .with_clob_price_history(history);
        let result = signal.compute(&ctx).await.unwrap();
        assert_eq!(result.direction, Direction::Down);
        assert!(result.strength > 0.5);
    }

    #[tokio::test]
    async fn fast_move_has_higher_strength_than_slow() {
        let mut signal = ClobVelocitySignal::new();

        // Fast: 0.50 → 0.56 in 10 seconds
        let fast_history = make_history(&[(0, 0.50), (10, 0.56)]);
        let ctx = SignalContext::new(Utc::now(), "BTCUSDT")
            .with_clob_price_history(fast_history);
        let fast_result = signal.compute(&ctx).await.unwrap();

        // Slow: 0.50 → 0.53 in 60 seconds
        let slow_history = make_history(&[(0, 0.50), (60, 0.53)]);
        let ctx = SignalContext::new(Utc::now(), "BTCUSDT")
            .with_clob_price_history(slow_history);
        let slow_result = signal.compute(&ctx).await.unwrap();

        assert!(
            fast_result.strength > slow_result.strength,
            "fast={} should > slow={}",
            fast_result.strength,
            slow_result.strength
        );
    }

    #[tokio::test]
    async fn strength_capped_at_one() {
        let mut signal = ClobVelocitySignal::new();
        // Extreme move: 0.50 → 0.70 in 5 seconds
        let history = make_history(&[(0, 0.50), (5, 0.70)]);
        let ctx = SignalContext::new(Utc::now(), "BTCUSDT")
            .with_clob_price_history(history);
        let result = signal.compute(&ctx).await.unwrap();
        assert!((result.strength - 1.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn metadata_contains_velocity() {
        let mut signal = ClobVelocitySignal::new();
        let history = make_history(&[(0, 0.50), (30, 0.55)]);
        let ctx = SignalContext::new(Utc::now(), "BTCUSDT")
            .with_clob_price_history(history);
        let result = signal.compute(&ctx).await.unwrap();
        assert!(result.metadata.contains_key("velocity_cents_per_sec"));
        assert!(result.metadata.contains_key("displacement"));
    }

    #[tokio::test]
    async fn lookback_window_respected() {
        let config = ClobVelocityConfig {
            lookback_secs: 30.0,
            ..ClobVelocityConfig::default()
        };
        let mut signal = ClobVelocitySignal::with_config(config);

        // Old data point at t=0, recent data at t=50 and t=60
        // With 30s lookback, should only use t=50 and t=60
        let history = make_history(&[
            (0, 0.40),  // old — outside lookback
            (50, 0.52), // within lookback
            (60, 0.56), // latest
        ]);
        let ctx = SignalContext::new(Utc::now(), "BTCUSDT")
            .with_clob_price_history(history);
        let result = signal.compute(&ctx).await.unwrap();

        // Velocity should be based on 0.52→0.56 over 10s, not 0.40→0.56 over 60s
        let vel = result.metadata.get("velocity_cents_per_sec").unwrap();
        // 4 cents / 10 seconds = 0.4 cents/sec
        assert!(
            (*vel - 0.4).abs() < 0.05,
            "expected ~0.4 cents/sec, got {}",
            vel
        );
    }

    #[test]
    fn default_config_has_reasonable_values() {
        let config = ClobVelocityConfig::default();
        assert!((config.lookback_secs - 60.0).abs() < f64::EPSILON);
        assert!((config.reference_velocity - 0.2).abs() < f64::EPSILON);
        assert!((config.min_displacement - 0.015).abs() < f64::EPSILON);
        assert!((config.weight - 1.3).abs() < f64::EPSILON);
    }
}
