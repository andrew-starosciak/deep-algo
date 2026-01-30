# Common Patterns (Statistical Trading Engine)

## Signal Generator Pattern

```rust
use async_trait::async_trait;

#[async_trait]
pub trait SignalGenerator: Send + Sync {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue>;
    fn name(&self) -> &str;
    fn weight(&self) -> f64 { 1.0 }
}

pub struct SignalValue {
    pub direction: Direction,
    pub strength: f64,        // 0.0 to 1.0
    pub confidence: f64,      // Statistical confidence
    pub metadata: HashMap<String, f64>,
}

// Example implementation
pub struct OrderBookImbalanceSignal {
    lookback: Duration,
    threshold: f64,
}

#[async_trait]
impl SignalGenerator for OrderBookImbalanceSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        let imbalance = ctx.orderbook.calculate_imbalance();
        let direction = if imbalance > self.threshold {
            Direction::Up
        } else if imbalance < -self.threshold {
            Direction::Down
        } else {
            Direction::Neutral
        };

        Ok(SignalValue {
            direction,
            strength: imbalance.abs().min(1.0),
            confidence: 0.0,  // Set by validation
            metadata: HashMap::new(),
        })
    }

    fn name(&self) -> &str { "order_book_imbalance" }
}
```

## Composite Signal Pattern

```rust
pub struct CompositeSignal {
    generators: Vec<Box<dyn SignalGenerator>>,
    combination: CombinationMethod,
}

pub enum CombinationMethod {
    Voting,           // Majority vote
    WeightedAverage,  // Sum of weighted signals
    Bayesian,         // Posterior probability
}

impl CompositeSignal {
    pub async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        let signals: Vec<SignalValue> = futures::future::try_join_all(
            self.generators.iter_mut().map(|g| g.compute(ctx))
        ).await?;

        match self.combination {
            CombinationMethod::Voting => self.vote(signals),
            CombinationMethod::WeightedAverage => self.weighted_average(signals),
            CombinationMethod::Bayesian => self.bayesian(signals),
        }
    }
}
```

## Actor Pattern (Bot Management)

```rust
use tokio::sync::mpsc;

pub enum BotCommand {
    Start,
    Stop,
    UpdateConfig(BotConfig),
    GetStatus(oneshot::Sender<BotStatus>),
}

pub struct BotActor {
    rx: mpsc::Receiver<BotCommand>,
    config: BotConfig,
    state: BotState,
}

impl BotActor {
    pub fn spawn(config: BotConfig) -> BotHandle {
        let (tx, rx) = mpsc::channel(32);
        let actor = Self { rx, config, state: BotState::Stopped };
        tokio::spawn(actor.run());
        BotHandle { tx }
    }

    async fn run(mut self) {
        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                BotCommand::Start => self.start().await,
                BotCommand::Stop => self.stop().await,
                BotCommand::UpdateConfig(c) => self.config = c,
                BotCommand::GetStatus(tx) => { let _ = tx.send(self.status()); }
            }
        }
    }
}

#[derive(Clone)]
pub struct BotHandle {
    tx: mpsc::Sender<BotCommand>,
}
```

## Statistical Validation Pattern

```rust
pub struct SignalValidator {
    min_samples: usize,
    significance_level: f64,
}

impl SignalValidator {
    pub fn validate(&self, predictions: &[Prediction], outcomes: &[Outcome]) -> ValidationResult {
        let n = predictions.len();
        if n < self.min_samples {
            return ValidationResult::InsufficientData { required: self.min_samples, actual: n };
        }

        let wins = predictions.iter().zip(outcomes)
            .filter(|(p, o)| p.direction == o.direction)
            .count();

        let win_rate = wins as f64 / n as f64;
        let (ci_lower, ci_upper) = wilson_ci(wins, n, 1.96);
        let p_value = binomial_test(wins, n, 0.5);

        ValidationResult::Complete {
            win_rate,
            wilson_ci: (ci_lower, ci_upper),
            p_value,
            significant: p_value < self.significance_level,
        }
    }
}

fn wilson_ci(wins: usize, n: usize, z: f64) -> (f64, f64) {
    let p = wins as f64 / n as f64;
    let n_f = n as f64;
    let denom = 1.0 + z.powi(2) / n_f;
    let center = p + z.powi(2) / (2.0 * n_f);
    let spread = z * (p * (1.0 - p) / n_f + z.powi(2) / (4.0 * n_f.powi(2))).sqrt();
    ((center - spread) / denom, (center + spread) / denom)
}
```

## Kelly Criterion Pattern

```rust
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

pub struct KellySizer {
    fraction: Decimal,  // 0.25 for quarter Kelly
    max_bet: Decimal,
    min_edge: Decimal,
}

impl KellySizer {
    pub fn size(&self, p: Decimal, price: Decimal, bankroll: Decimal) -> Option<Decimal> {
        // Net odds: b = (1 - price) / price
        let b = (Decimal::ONE - price) / price;

        // Full Kelly: f* = (p(b+1) - 1) / b
        let full_kelly = (p * (b + Decimal::ONE) - Decimal::ONE) / b;

        // Check minimum edge
        let ev = p * (Decimal::ONE - price) - (Decimal::ONE - p) * price;
        if ev < self.min_edge {
            return None;
        }

        // Apply fraction and caps
        let bet = (full_kelly * self.fraction * bankroll)
            .min(self.max_bet)
            .max(Decimal::ZERO);

        Some(bet)
    }
}
```

## Database Batch Insert Pattern

```rust
pub async fn insert_orderbook_batch(
    pool: &PgPool,
    snapshots: &[OrderBookSnapshot],
) -> Result<()> {
    let mut tx = pool.begin().await?;

    for chunk in snapshots.chunks(100) {
        sqlx::query!(
            r#"
            INSERT INTO orderbook_snapshots (timestamp, symbol, exchange, bid_levels, ask_levels, imbalance)
            SELECT * FROM UNNEST($1::timestamptz[], $2::text[], $3::text[], $4::jsonb[], $5::jsonb[], $6::decimal[])
            "#,
            &chunk.iter().map(|s| s.timestamp).collect::<Vec<_>>(),
            &chunk.iter().map(|s| &s.symbol).collect::<Vec<_>>(),
            &chunk.iter().map(|s| &s.exchange).collect::<Vec<_>>(),
            &chunk.iter().map(|s| &s.bid_levels).collect::<Vec<_>>(),
            &chunk.iter().map(|s| &s.ask_levels).collect::<Vec<_>>(),
            &chunk.iter().map(|s| s.imbalance).collect::<Vec<_>>(),
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}
```
