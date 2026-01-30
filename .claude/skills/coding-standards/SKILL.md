---
name: coding-standards
description: Rust coding standards for statistical trading systems. Financial precision, error handling, async patterns, and code organization.
---

# Coding Standards (Rust)

Standards for writing reliable, maintainable trading system code.

## When to Activate

- Writing any Rust code in this project
- Reviewing code for quality
- Refactoring existing code
- Setting up new modules

## Financial Precision (CRITICAL)

**ALWAYS use `rust_decimal::Decimal` for money:**

```rust
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

// CORRECT
let price: Decimal = dec!(42750.50);
let stake: Decimal = "100.00".parse()?;
let fee = price * dec!(0.001);

// WRONG - NEVER use floats for money
let price: f64 = 42750.50;  // Precision loss!
```

### When to Use Each Type

| Type | Use For |
|------|---------|
| `Decimal` | Prices, quantities, P&L, fees, any money |
| `f64` | Signal strength (0.0-1.0), confidence scores, statistics |
| `i64` | Counts, IDs, timestamps (millis) |

## Error Handling

### No Panics in Library Code

```rust
// WRONG
let value = some_operation().unwrap();
let value = some_operation().expect("should work");

// CORRECT
let value = some_operation()?;

// CORRECT (when you must handle locally)
let value = match some_operation() {
    Ok(v) => v,
    Err(e) => {
        tracing::error!("Operation failed: {e}");
        return Err(e.into());
    }
};
```

### Custom Error Types

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SignalError {
    #[error("insufficient data: need {required}, have {actual}")]
    InsufficientData { required: usize, actual: usize },

    #[error("invalid threshold: {0}")]
    InvalidThreshold(f64),

    #[error("computation failed: {0}")]
    ComputationFailed(#[from] std::io::Error),
}
```

## Async Patterns

### Actor Pattern for Long-Running Tasks

```rust
use tokio::sync::mpsc;

pub struct SignalCollector {
    rx: mpsc::Receiver<CollectorCommand>,
    db: DatabaseClient,
}

impl SignalCollector {
    pub fn spawn(db: DatabaseClient) -> CollectorHandle {
        let (tx, rx) = mpsc::channel(100);
        let collector = Self { rx, db };
        tokio::spawn(collector.run());
        CollectorHandle { tx }
    }

    async fn run(mut self) {
        while let Some(cmd) = self.rx.recv().await {
            if let Err(e) = self.handle(cmd).await {
                tracing::error!("Command failed: {e}");
            }
        }
    }
}
```

### Graceful Shutdown

```rust
use tokio::signal;

pub async fn run_with_shutdown<F>(task: F) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    tokio::select! {
        result = task => result,
        _ = signal::ctrl_c() => {
            tracing::info!("Shutdown signal received");
            Ok(())
        }
    }
}
```

## Code Organization

### File Size Guidelines

- **Target:** 200-400 lines
- **Maximum:** 800 lines
- **Split when:** >500 lines or multiple responsibilities

### Module Structure

```
crates/signals/src/
├── lib.rs              # Public API exports
├── traits.rs           # SignalGenerator trait
├── types.rs            # SignalValue, Direction, etc.
├── orderbook/
│   ├── mod.rs          # Module exports
│   ├── imbalance.rs    # OrderBookImbalanceSignal
│   └── wall.rs         # WallDetectionSignal
├── funding/
│   ├── mod.rs
│   ├── reversal.rs     # FundingReversalSignal
│   └── percentile.rs   # Percentile calculation
└── composite.rs        # CompositeSignal
```

### Public API Design

```rust
// lib.rs - Clean public exports
pub mod traits;
pub mod types;

// Re-export commonly used items
pub use traits::SignalGenerator;
pub use types::{SignalValue, Direction, SignalContext};

// Specific signals under feature-gated modules
pub mod orderbook;
pub mod funding;
pub mod liquidation;
pub mod composite;
```

## Documentation

### Required for Public Items

```rust
/// Generates signals based on order book imbalance.
///
/// The signal compares bid and ask volumes to detect
/// directional pressure in the market.
///
/// # Example
///
/// ```rust
/// let signal = OrderBookImbalanceSignal::new(1.5);
/// let result = signal.compute(&ctx).await?;
/// ```
///
/// # Errors
///
/// Returns `SignalError::InsufficientData` if the order book
/// has fewer than 5 price levels on either side.
pub struct OrderBookImbalanceSignal {
    threshold: Decimal,
}
```

## Testing Standards

### Test Organization

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Group by behavior
    mod when_bid_heavy {
        use super::*;

        #[test]
        fn signals_up() { /* ... */ }

        #[test]
        fn strength_proportional_to_imbalance() { /* ... */ }
    }

    mod when_ask_heavy {
        use super::*;

        #[test]
        fn signals_down() { /* ... */ }
    }

    mod edge_cases {
        use super::*;

        #[test]
        fn handles_zero_volume() { /* ... */ }

        #[test]
        fn handles_equal_volumes() { /* ... */ }
    }
}
```

## Naming Conventions

| Item | Convention | Example |
|------|------------|---------|
| Structs | PascalCase | `OrderBookImbalanceSignal` |
| Traits | PascalCase | `SignalGenerator` |
| Functions | snake_case | `compute_imbalance` |
| Constants | SCREAMING_SNAKE | `MAX_LOOKBACK_SECONDS` |
| Modules | snake_case | `order_book` |
| Type params | Single uppercase | `T`, `E` |

## Imports

### Organize by Category

```rust
// Standard library
use std::collections::HashMap;
use std::time::Duration;

// External crates
use rust_decimal::Decimal;
use tokio::sync::mpsc;
use tracing::{info, error};

// Workspace crates
use algo_trade_core::traits::SignalGenerator;
use algo_trade_data::DatabaseClient;

// Local modules
use crate::types::{SignalValue, Direction};
```

## Pre-Commit Checklist

Before every commit:

```bash
# Format
cargo fmt

# Lint
cargo clippy -- -D warnings

# Test
cargo test

# Check for unwrap/expect in library code
rg "\.unwrap\(\)|\.expect\(" crates/*/src --type rust
```

## Anti-Patterns to Avoid

### 1. Mutation Over Creation

```rust
// WRONG
fn update_signal(signal: &mut Signal) {
    signal.strength = 0.8;
}

// CORRECT
fn with_strength(signal: Signal, strength: f64) -> Signal {
    Signal { strength, ..signal }
}
```

### 2. Stringly-Typed APIs

```rust
// WRONG
fn get_signal(name: &str) -> Signal;

// CORRECT
fn get_signal(signal_type: SignalType) -> Signal;

enum SignalType {
    OrderBookImbalance,
    FundingRate,
    Liquidation,
}
```

### 3. God Objects

```rust
// WRONG - Does too much
struct TradingEngine {
    signals: Vec<Signal>,
    database: Database,
    exchange: Exchange,
    risk: RiskManager,
    // ... 20 more fields
}

// CORRECT - Single responsibility
struct SignalAggregator { /* signals only */ }
struct DataCollector { /* data only */ }
struct RiskManager { /* risk only */ }
```

### 4. Ignored Errors

```rust
// WRONG
let _ = risky_operation();

// CORRECT
if let Err(e) = risky_operation() {
    tracing::warn!("Operation failed (non-fatal): {e}");
}
```
