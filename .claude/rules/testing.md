# Testing Requirements (Rust)

## Statistical Validation First

Before implementing any signal, validate statistically:

```rust
// Signal must demonstrate:
pub struct SignalValidation {
    pub p_value: f64,              // Must be < 0.05
    pub sample_size: usize,        // Must be > 100
    pub correlation: f64,          // With outcome
    pub information_coefficient: f64,
}
```

## Test Structure

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_computes_correct_direction() {
        // Arrange
        let ctx = SignalContext::mock();
        let mut signal = OrderBookImbalanceSignal::new();

        // Act
        let result = signal.compute_sync(&ctx).unwrap();

        // Assert
        assert_eq!(result.direction, Direction::Up);
        assert!(result.strength > 0.5);
    }

    #[tokio::test]
    async fn async_operation_succeeds() {
        // Async test example
        let result = fetch_data().await;
        assert!(result.is_ok());
    }
}
```

## Test Categories

### Unit Tests
- Test individual functions and methods
- Mock external dependencies
- Fast execution (<1s per test)

### Integration Tests
- Test database operations
- Test API endpoints
- Use test containers when needed

### Statistical Tests
- Validate signal predictive power
- Test confidence intervals
- Verify hypothesis testing logic

## Backtest Validation

Binary outcome backtests must report:

```rust
pub struct BacktestReport {
    pub total_bets: u32,
    pub win_rate: f64,
    pub wilson_ci: (f64, f64),     // 95% CI
    pub binomial_p_value: f64,     // H0: p = 0.50
    pub ev_per_bet: Decimal,
}
```

## Go/No-Go Thresholds

| Metric | Minimum Threshold |
|--------|-------------------|
| Signal p-value | < 0.10 (development), < 0.05 (production) |
| Backtest win rate | > 53% |
| Sample size | > 100 bets |
| Walk-forward validation | Positive EV in out-of-sample |

## Test Commands

```bash
# Run all tests
cargo test

# Run specific crate tests
cargo test -p algo-trade-signals

# Run with output
cargo test -- --nocapture

# Run integration tests only
cargo test --test '*'

# Run statistical validation
cargo run -p algo-trade-cli -- validate-signals --start 2025-01-01 --end 2025-01-28
```

## Property-Based Testing

Use `proptest` for edge case discovery:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn kelly_never_exceeds_one(p in 0.0..1.0, b in 0.01..10.0) {
        let kelly = calculate_kelly(p, b);
        prop_assert!(kelly <= 1.0);
    }
}
```
