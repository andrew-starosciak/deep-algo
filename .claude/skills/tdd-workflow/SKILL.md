---
name: tdd-workflow
description: Test-driven development for Rust. Write tests first, implement to pass, verify statistical correctness. Enforces 80%+ coverage.
---

# Test-Driven Development Workflow (Rust)

This skill ensures all code development follows TDD principles with comprehensive test coverage.

## When to Activate

- Writing new signal generators
- Implementing statistical calculations
- Adding exchange integrations
- Creating risk management logic
- Fixing bugs in trading logic

## Core Principles

### 1. Tests BEFORE Code
ALWAYS write tests first, then implement code to make tests pass.

### 2. Coverage Requirements
- Minimum 80% coverage
- All edge cases covered
- Error scenarios tested
- Statistical correctness verified

### 3. Test Categories

#### Unit Tests
- Individual signal computations
- Statistical calculations (Wilson CI, Kelly)
- Data transformations

#### Integration Tests
- Database operations
- WebSocket connections
- API client behavior

#### Property-Based Tests
- Statistical invariants
- Financial calculation bounds

## TDD Workflow Steps

### Step 1: Define Behavior
```rust
// Signal should detect order book imbalance
// When bid_volume > ask_volume * 1.5, signal Up
// When ask_volume > bid_volume * 1.5, signal Down
// Otherwise, signal Neutral
```

### Step 2: Write Failing Test
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_up_when_bid_heavy() {
        let ctx = SignalContext {
            orderbook: OrderBook {
                bid_volume: dec!(150),
                ask_volume: dec!(100),
            },
            ..Default::default()
        };

        let mut signal = OrderBookImbalanceSignal::new(1.5);
        let result = signal.compute_sync(&ctx).unwrap();

        assert_eq!(result.direction, Direction::Up);
    }

    #[test]
    fn signal_down_when_ask_heavy() {
        let ctx = SignalContext {
            orderbook: OrderBook {
                bid_volume: dec!(100),
                ask_volume: dec!(150),
            },
            ..Default::default()
        };

        let mut signal = OrderBookImbalanceSignal::new(1.5);
        let result = signal.compute_sync(&ctx).unwrap();

        assert_eq!(result.direction, Direction::Down);
    }

    #[test]
    fn signal_neutral_when_balanced() {
        let ctx = SignalContext {
            orderbook: OrderBook {
                bid_volume: dec!(100),
                ask_volume: dec!(100),
            },
            ..Default::default()
        };

        let mut signal = OrderBookImbalanceSignal::new(1.5);
        let result = signal.compute_sync(&ctx).unwrap();

        assert_eq!(result.direction, Direction::Neutral);
    }
}
```

### Step 3: Run Tests (Should Fail)
```bash
cargo test -p algo-trade-signals
# Tests fail - not implemented yet
```

### Step 4: Implement Minimal Code
```rust
pub struct OrderBookImbalanceSignal {
    threshold: Decimal,
}

impl OrderBookImbalanceSignal {
    pub fn new(threshold: f64) -> Self {
        Self { threshold: Decimal::from_f64(threshold).unwrap() }
    }

    pub fn compute_sync(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        let ratio = ctx.orderbook.bid_volume / ctx.orderbook.ask_volume;

        let direction = if ratio > self.threshold {
            Direction::Up
        } else if ratio < Decimal::ONE / self.threshold {
            Direction::Down
        } else {
            Direction::Neutral
        };

        Ok(SignalValue {
            direction,
            strength: (ratio - Decimal::ONE).abs().min(Decimal::ONE).to_f64().unwrap(),
            confidence: 0.0,
            metadata: HashMap::new(),
        })
    }
}
```

### Step 5: Run Tests (Should Pass)
```bash
cargo test -p algo-trade-signals
# All tests pass
```

### Step 6: Add Edge Cases
```rust
#[test]
fn handles_zero_ask_volume() {
    let ctx = SignalContext {
        orderbook: OrderBook {
            bid_volume: dec!(100),
            ask_volume: Decimal::ZERO,
        },
        ..Default::default()
    };

    let mut signal = OrderBookImbalanceSignal::new(1.5);
    let result = signal.compute_sync(&ctx);

    // Should handle gracefully, not panic
    assert!(result.is_err() || result.unwrap().direction == Direction::Up);
}
```

### Step 7: Verify Coverage
```bash
cargo tarpaulin -p algo-trade-signals --out Html
# Open tarpaulin-report.html
```

## Statistical Test Patterns

### Testing Confidence Intervals
```rust
#[test]
fn wilson_ci_contains_true_proportion() {
    // 550 wins out of 1000 trials (55% win rate)
    let (lower, upper) = wilson_ci(550, 1000, 1.96);

    // CI should contain the true proportion
    assert!(lower < 0.55);
    assert!(upper > 0.55);

    // CI should be reasonable width
    assert!(upper - lower < 0.10);
}

#[test]
fn wilson_ci_narrows_with_more_samples() {
    let (lower1, upper1) = wilson_ci(55, 100, 1.96);
    let (lower2, upper2) = wilson_ci(550, 1000, 1.96);

    let width1 = upper1 - lower1;
    let width2 = upper2 - lower2;

    // More samples = narrower CI
    assert!(width2 < width1);
}
```

### Testing Kelly Criterion
```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn kelly_never_exceeds_one(p in 0.01..0.99f64, b in 0.01..10.0f64) {
        let kelly = calculate_kelly(p, b);
        prop_assert!(kelly <= 1.0);
    }

    #[test]
    fn kelly_negative_when_no_edge(p in 0.01..0.49f64, b in 0.5..2.0f64) {
        // When p < 1/(b+1), Kelly should be negative (no bet)
        if p < 1.0 / (b + 1.0) {
            let kelly = calculate_kelly(p, b);
            prop_assert!(kelly < 0.0);
        }
    }

    #[test]
    fn kelly_increases_with_edge(b in 1.0..2.0f64) {
        let k1 = calculate_kelly(0.55, b);
        let k2 = calculate_kelly(0.60, b);
        prop_assert!(k2 > k1);
    }
}
```

### Testing Financial Calculations
```rust
#[test]
fn ev_calculation_matches_formula() {
    let p = dec!(0.55);      // Win probability
    let price = dec!(0.45);  // Cost per share

    // EV = p * (1 - price) - (1-p) * price
    let expected_ev = p * (Decimal::ONE - price) - (Decimal::ONE - p) * price;
    let calculated_ev = calculate_ev(p, price);

    assert_eq!(calculated_ev, expected_ev);
}

#[test]
fn no_bet_when_negative_ev() {
    let p = dec!(0.45);      // Below break-even
    let price = dec!(0.50);

    let ev = calculate_ev(p, price);
    assert!(ev < Decimal::ZERO);

    let bet = should_bet(p, price, dec!(0.02)); // 2% minimum edge
    assert!(!bet);
}
```

## Async Test Patterns

```rust
#[tokio::test]
async fn signal_computes_async() {
    let ctx = SignalContext::mock();
    let mut signal = FundingRateSignal::new();

    let result = signal.compute(&ctx).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn handles_timeout() {
    let slow_provider = SlowMockProvider::new(Duration::from_secs(10));

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        fetch_data(&slow_provider)
    ).await;

    assert!(result.is_err()); // Should timeout
}
```

## Database Test Patterns

```rust
#[sqlx::test]
async fn inserts_orderbook_snapshot(pool: PgPool) {
    let snapshot = OrderBookSnapshot {
        timestamp: Utc::now(),
        symbol: "BTCUSDT".to_string(),
        exchange: "binance".to_string(),
        bid_volume: dec!(100),
        ask_volume: dec!(100),
        imbalance: dec!(0),
    };

    insert_snapshot(&pool, &snapshot).await.unwrap();

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM orderbook_snapshots")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(count.0, 1);
}
```

## Test Commands

```bash
# Run all tests
cargo test

# Run specific crate
cargo test -p algo-trade-signals

# Run with output
cargo test -- --nocapture

# Run specific test
cargo test wilson_ci_contains_true_proportion

# Coverage report
cargo tarpaulin --out Html

# Property tests with more cases
cargo test -- --test-threads=1
```

## Coverage Verification

```toml
# In Cargo.toml or .cargo/config.toml
[target.'cfg(coverage)'.coverage]
exclude = [
    "tests/*",
    "benches/*",
]
```

## Success Metrics

- 80%+ code coverage
- All tests passing
- Property tests cover edge cases
- Statistical calculations verified
- No `unwrap()` in library code
- Async tests use proper timeouts
