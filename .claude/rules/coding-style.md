# Coding Style (Rust)

## Financial Precision (CRITICAL)

**ALWAYS use `rust_decimal::Decimal` for financial values:**

```rust
// WRONG: Float accumulates errors
let price: f64 = 42750.50;
let total = price * 100.0;  // Precision loss!

// CORRECT: Decimal for all money
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

let price: Decimal = dec!(42750.50);
let total = price * dec!(100);  // Exact precision
```

## Immutability & Ownership

Prefer immutable references and owned data:

```rust
// WRONG: Unnecessary mutation
fn update_signal(signal: &mut Signal) {
    signal.strength = 0.8;
}

// CORRECT: Return new value
fn with_strength(signal: Signal, strength: f64) -> Signal {
    Signal { strength, ..signal }
}
```

## Error Handling

Use `Result<T, E>` with proper error types:

```rust
// WRONG: Panic on error
let value = some_operation().unwrap();

// CORRECT: Propagate with ?
let value = some_operation()?;

// CORRECT: Handle explicitly when needed
match some_operation() {
    Ok(v) => process(v),
    Err(e) => {
        tracing::error!("Operation failed: {e}");
        return Err(e.into());
    }
}
```

## File Organization

- **200-400 lines typical**, 800 max per file
- One struct/trait per file for complex types
- Group related functionality in modules
- Use `mod.rs` sparingly - prefer `module_name.rs`

## Async Patterns

All async code uses Tokio:

```rust
// Actor pattern for long-running tasks
pub struct BotActor {
    rx: mpsc::Receiver<Command>,
    // ...
}

impl BotActor {
    pub async fn run(mut self) {
        while let Some(cmd) = self.rx.recv().await {
            self.handle(cmd).await;
        }
    }
}
```

## Trait Design

Keep traits focused and composable:

```rust
// GOOD: Single responsibility
pub trait SignalGenerator: Send + Sync {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue>;
    fn name(&self) -> &str;
}

// BAD: God trait
pub trait TradingSystem {
    fn generate_signal(&self);
    fn execute_order(&self);
    fn calculate_risk(&self);
    fn store_data(&self);
}
```

## Code Quality Checklist

Before marking work complete:
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo fmt` applied
- [ ] No `unwrap()` or `expect()` in library code
- [ ] All `pub` items have doc comments
- [ ] Financial values use `Decimal`
- [ ] Async code uses proper error handling
- [ ] Tests cover happy path and error cases
