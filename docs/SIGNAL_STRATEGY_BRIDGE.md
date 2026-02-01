# Signal-Strategy Bridge Architecture

## Executive Summary

This document describes a bridging architecture that enables microstructure `SignalGenerator` implementations (LiquidationCascadeSignal, FundingRateSignal, OrderBookImbalanceSignal, etc.) to be used alongside traditional `Strategy` implementations (MACrossover, QuadMA) for perpetual trading on Hyperliquid.

The bridge enables four key use cases:
1. **Entry Filters**: Block entries when microstructure signals conflict with strategy direction
2. **Exit Triggers**: Force exits based on extreme microstructure conditions
3. **Position Sizing**: Adjust size based on market stress indicators
4. **Entry Timing**: Delay entries until order book support materializes

---

## Current Architecture Analysis

### SignalGenerator (Polymarket Binary Bets)

**Location**: `crates/core/src/signal.rs`, `crates/signals/src/generator/`

```rust
#[async_trait]
pub trait SignalGenerator: Send + Sync {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue>;
    fn name(&self) -> &str;
    fn weight(&self) -> f64 { 1.0 }
}

pub struct SignalValue {
    pub direction: Direction,    // Up, Down, Neutral
    pub strength: f64,           // 0.0 to 1.0
    pub confidence: f64,         // Statistical confidence
    pub metadata: HashMap<String, f64>,
}
```

**Key characteristics**:
- Async execution (database queries, API calls)
- Requires `SignalContext` with historical data (order book, funding, liquidations, news)
- Outputs directional probability estimates (Up/Down/Neutral)
- Designed for point-in-time binary outcome prediction

### Strategy (Hyperliquid Perpetuals)

**Location**: `crates/core/src/traits.rs`, `crates/strategy/src/`

```rust
#[async_trait]
pub trait Strategy: Send + Sync {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>>;
    fn name(&self) -> &'static str;
}

pub struct SignalEvent {
    pub symbol: String,
    pub direction: SignalDirection,  // Long, Short, Exit
    pub strength: f64,
    pub price: Decimal,
    pub timestamp: DateTime<Utc>,
}
```

**Key characteristics**:
- Event-driven (processes MarketEvent stream)
- Maintains internal state (MA buffers, position tracking)
- Outputs position signals (Long/Short/Exit)
- Designed for continuous perpetual trading

### Key Differences

| Aspect | SignalGenerator | Strategy |
|--------|-----------------|----------|
| Execution | Async (point-in-time) | Event-driven (stream) |
| Input | SignalContext (rich historical) | MarketEvent (single event) |
| Output | Direction probability | Position signal |
| State | Minimal (in context) | Internal buffers |
| Direction enum | Up/Down/Neutral | Long/Short/Exit |

---

## Proposed Architecture

### Core Design Principles

1. **Composition over modification**: Wrap existing traits rather than change signatures
2. **Async bridge**: Handle sync/async boundary cleanly via cached state
3. **Minimal coupling**: Each system can operate independently
4. **Configurable behavior**: Entry filter, exit trigger, sizing, and timing are all optional

### Architecture Diagram

```
                                    +----------------------------------+
                                    |    MicrostructureOrchestrator    |
                                    |  (caches signal values async)    |
                                    +-----------------+----------------+
                                                      |
                                                      | spawn background task
                                                      v
+-------------------+            +------------------------------------------+
|  SignalContext    |----------->|        Signal Collection Loop            |
|    Builder        |            |  (polls DB every N seconds)              |
+-------------------+            +-------------------+----------------------+
                                                      |
                                                      | updates cached values
                                                      v
                                    +----------------------------------+
                                    |      CachedMicroSignals          |
                                    |  order_book_imbalance: SignalValue|
                                    |  funding_rate: SignalValue        |
                                    |  liquidation_cascade: SignalValue |
                                    |  news: SignalValue                |
                                    |  last_updated: DateTime           |
                                    +----------------------------------+
                                                      ^
                                                      | read (sync via RwLock)
                                                      |
+-------------------+            +------------------------------------------+
|   MarketEvent     |----------->|      EnhancedStrategy<S: Strategy>      |
|     Stream        |            |  +------------------------------------+ |
+-------------------+            |  |  Inner Strategy (MACrossover)      | |
                                 |  +------------------------------------+ |
                                 |  +------------------------------------+ |
                                 |  |  MicrostructureFilterConfig        | |
                                 |  |  - entry_filter: bool              | |
                                 |  |  - exit_trigger: bool              | |
                                 |  |  - sizing_adjustment: bool         | |
                                 |  |  - entry_timing: bool              | |
                                 |  +------------------------------------+ |
                                 +-------------------+----------------------+
                                                      |
                                                      | outputs
                                                      v
                                    +----------------------------------+
                                    |  SignalEvent (possibly modified) |
                                    |  - blocked by filter             |
                                    |  - forced exit triggered         |
                                    |  - strength adjusted for sizing  |
                                    +----------------------------------+
```

---

## New Types

### 1. CachedMicroSignals - Shared Microstructure State

```rust
// crates/signals/src/bridge/cached_signals.rs

use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use algo_trade_core::{Direction, SignalValue};

/// Cached microstructure signals for sync access from strategies.
/// Updated asynchronously by background collector task.
#[derive(Debug, Clone)]
pub struct CachedMicroSignals {
    pub order_book_imbalance: SignalValue,
    pub funding_rate: SignalValue,
    pub liquidation_cascade: SignalValue,
    pub news: SignalValue,
    pub composite: SignalValue,
    pub last_updated: DateTime<Utc>,
}

impl Default for CachedMicroSignals {
    fn default() -> Self {
        Self {
            order_book_imbalance: SignalValue::neutral(),
            funding_rate: SignalValue::neutral(),
            liquidation_cascade: SignalValue::neutral(),
            news: SignalValue::neutral(),
            composite: SignalValue::neutral(),
            last_updated: Utc::now(),
        }
    }
}

/// Thread-safe handle to cached signals
pub type SharedMicroSignals = Arc<RwLock<CachedMicroSignals>>;

impl CachedMicroSignals {
    /// Returns true if any signal indicates high market stress
    pub fn is_high_stress(&self) -> bool {
        let liquidation_stress = self.liquidation_cascade.strength > 0.7;
        let funding_extreme = self.funding_rate.strength > 0.8;
        liquidation_stress || funding_extreme
    }

    /// Returns the dominant direction across all signals
    pub fn consensus_direction(&self) -> Direction {
        let mut up_weight = 0.0;
        let mut down_weight = 0.0;

        for signal in [
            &self.order_book_imbalance,
            &self.funding_rate,
            &self.liquidation_cascade,
        ] {
            match signal.direction {
                Direction::Up => up_weight += signal.strength,
                Direction::Down => down_weight += signal.strength,
                Direction::Neutral => {}
            }
        }

        if up_weight > down_weight && up_weight > 0.5 {
            Direction::Up
        } else if down_weight > up_weight && down_weight > 0.5 {
            Direction::Down
        } else {
            Direction::Neutral
        }
    }

    /// Check if microstructure supports a given strategy direction
    pub fn supports_direction(&self, direction: &SignalDirection) -> bool {
        let micro_dir = self.consensus_direction();
        match (direction, micro_dir) {
            (SignalDirection::Long, Direction::Up) => true,
            (SignalDirection::Short, Direction::Down) => true,
            (_, Direction::Neutral) => true, // Neutral does not block
            (SignalDirection::Exit, _) => true, // Exits always allowed
            _ => false, // Direction conflict
        }
    }
}
```

### 2. MicrostructureOrchestrator - Background Signal Collection

```rust
// crates/signals/src/bridge/orchestrator.rs

use tokio::sync::mpsc;
use std::time::Duration;
use sqlx::PgPool;

/// Commands to control the orchestrator
pub enum OrchestratorCommand {
    UpdateNow,
    Shutdown,
}

/// Background task that periodically updates microstructure signals
pub struct MicrostructureOrchestrator {
    pool: PgPool,
    symbol: String,
    exchange: String,
    signals: SharedMicroSignals,
    update_interval: Duration,
    generators: MicrostructureGenerators,
}

/// Collection of configured signal generators
pub struct MicrostructureGenerators {
    pub order_book: OrderBookImbalanceSignal,
    pub funding: FundingRateSignal,
    pub liquidation: LiquidationCascadeSignal,
    pub news: NewsSignal,
    pub composite: CompositeSignal,
}

impl MicrostructureOrchestrator {
    pub fn new(
        pool: PgPool,
        symbol: &str,
        exchange: &str,
        signals: SharedMicroSignals,
    ) -> Self {
        Self {
            pool,
            symbol: symbol.to_string(),
            exchange: exchange.to_string(),
            signals,
            update_interval: Duration::from_secs(5),
            generators: MicrostructureGenerators::default(),
        }
    }

    /// Spawns background collection task, returns command channel
    pub fn spawn(mut self) -> mpsc::Sender<OrchestratorCommand> {
        let (tx, mut rx) = mpsc::channel(32);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(self.update_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = self.update_signals().await {
                            tracing::warn!("Failed to update microstructure signals: {e}");
                        }
                    }
                    Some(cmd) = rx.recv() => {
                        match cmd {
                            OrchestratorCommand::UpdateNow => {
                                let _ = self.update_signals().await;
                            }
                            OrchestratorCommand::Shutdown => {
                                tracing::info!("Microstructure orchestrator shutting down");
                                break;
                            }
                        }
                    }
                }
            }
        });

        tx
    }

    async fn update_signals(&mut self) -> Result<()> {
        let now = Utc::now();

        // Build context from database
        let ctx = SignalContextBuilder::new(
            self.pool.clone(),
            &self.symbol,
            &self.exchange,
        )
        .build_at(now)
        .await?;

        // Compute all signals
        let ob_signal = self.generators.order_book.compute(&ctx).await?;
        let funding_signal = self.generators.funding.compute(&ctx).await?;
        let liq_signal = self.generators.liquidation.compute(&ctx).await?;
        let news_signal = self.generators.news.compute(&ctx).await?;
        let composite_signal = self.generators.composite.compute(&ctx).await?;

        // Update cache under write lock
        {
            let mut cache = self.signals.write().await;
            cache.order_book_imbalance = ob_signal;
            cache.funding_rate = funding_signal;
            cache.liquidation_cascade = liq_signal;
            cache.news = news_signal;
            cache.composite = composite_signal;
            cache.last_updated = now;
        }

        Ok(())
    }
}
```

### 3. MicrostructureFilter - Decision Logic

```rust
// crates/signals/src/bridge/filter.rs

use rust_decimal::Decimal;
use algo_trade_core::events::{SignalDirection, SignalEvent};

/// Configuration for microstructure filtering
#[derive(Debug, Clone)]
pub struct MicrostructureFilterConfig {
    /// Block entries when microstructure direction conflicts
    pub entry_filter_enabled: bool,
    /// Minimum imbalance strength to block entry (0.0 to 1.0)
    pub entry_filter_threshold: f64,

    /// Force exit on extreme microstructure conditions
    pub exit_trigger_enabled: bool,
    /// Liquidation cascade strength to trigger exit
    pub exit_liquidation_threshold: f64,
    /// Funding rate strength to trigger exit
    pub exit_funding_threshold: f64,

    /// Adjust position size based on market stress
    pub sizing_adjustment_enabled: bool,
    /// Reduce size to this fraction under high stress (0.0 to 1.0)
    pub stress_size_multiplier: f64,

    /// Delay entry until order book supports direction
    pub entry_timing_enabled: bool,
    /// Minimum imbalance in favor of direction
    pub timing_support_threshold: f64,
}

impl Default for MicrostructureFilterConfig {
    fn default() -> Self {
        Self {
            entry_filter_enabled: true,
            entry_filter_threshold: 0.6,
            exit_trigger_enabled: true,
            exit_liquidation_threshold: 0.8,
            exit_funding_threshold: 0.9,
            sizing_adjustment_enabled: true,
            stress_size_multiplier: 0.5,
            entry_timing_enabled: false,  // Opt-in feature
            timing_support_threshold: 0.3,
        }
    }
}

/// Result of applying microstructure filter
#[derive(Debug, Clone)]
pub enum FilterResult {
    /// Allow signal to pass through unchanged
    Allow(SignalEvent),
    /// Block the signal entirely
    Block { reason: String },
    /// Modify the signal (e.g., reduce strength for sizing)
    Modify(SignalEvent),
    /// Override with forced exit
    ForceExit { reason: String, signal: SignalEvent },
}

/// Applies microstructure signals to filter/modify strategy signals
pub struct MicrostructureFilter {
    config: MicrostructureFilterConfig,
}

impl MicrostructureFilter {
    pub fn new(config: MicrostructureFilterConfig) -> Self {
        Self { config }
    }

    /// Apply filter to a strategy signal based on current microstructure state
    pub fn apply(
        &self,
        signal: SignalEvent,
        micro: &CachedMicroSignals,
    ) -> FilterResult {
        // Check for forced exit conditions first
        if self.config.exit_trigger_enabled {
            if let Some(exit_result) = self.check_exit_trigger(&signal, micro) {
                return exit_result;
            }
        }

        // Check entry filter
        if self.config.entry_filter_enabled && signal.direction != SignalDirection::Exit {
            if let Some(block_result) = self.check_entry_filter(&signal, micro) {
                return block_result;
            }
        }

        // Check entry timing
        if self.config.entry_timing_enabled && signal.direction != SignalDirection::Exit {
            if !self.check_entry_timing(&signal, micro) {
                return FilterResult::Block {
                    reason: "Waiting for order book support".to_string(),
                };
            }
        }

        // Apply sizing adjustment if enabled
        if self.config.sizing_adjustment_enabled {
            if let Some(modified) = self.apply_sizing_adjustment(signal.clone(), micro) {
                return FilterResult::Modify(modified);
            }
        }

        FilterResult::Allow(signal)
    }

    fn check_exit_trigger(
        &self,
        signal: &SignalEvent,
        micro: &CachedMicroSignals,
    ) -> Option<FilterResult> {
        // Check liquidation cascade against position direction
        let liq = &micro.liquidation_cascade;
        if liq.strength >= self.config.exit_liquidation_threshold {
            let should_exit = match (&signal.direction, liq.direction) {
                (SignalDirection::Long, Direction::Down) => true,
                (SignalDirection::Short, Direction::Up) => true,
                _ => false,
            };

            if should_exit {
                return Some(FilterResult::ForceExit {
                    reason: format!(
                        "Liquidation cascade against position (strength: {:.2})",
                        liq.strength
                    ),
                    signal: SignalEvent {
                        direction: SignalDirection::Exit,
                        strength: 1.0,
                        ..signal.clone()
                    },
                });
            }
        }

        // Check extreme funding rate
        let funding = &micro.funding_rate;
        if funding.strength >= self.config.exit_funding_threshold {
            let should_exit = match (&signal.direction, funding.direction) {
                (SignalDirection::Long, Direction::Down) => true,
                (SignalDirection::Short, Direction::Up) => true,
                _ => false,
            };

            if should_exit {
                return Some(FilterResult::ForceExit {
                    reason: format!(
                        "Extreme funding rate against position (strength: {:.2})",
                        funding.strength
                    ),
                    signal: SignalEvent {
                        direction: SignalDirection::Exit,
                        strength: 1.0,
                        ..signal.clone()
                    },
                });
            }
        }

        None
    }

    fn check_entry_filter(
        &self,
        signal: &SignalEvent,
        micro: &CachedMicroSignals,
    ) -> Option<FilterResult> {
        let composite = &micro.composite;

        if composite.strength < self.config.entry_filter_threshold {
            return None;
        }

        let conflicts = match (&signal.direction, composite.direction) {
            (SignalDirection::Long, Direction::Down) => true,
            (SignalDirection::Short, Direction::Up) => true,
            _ => false,
        };

        if conflicts {
            Some(FilterResult::Block {
                reason: format!(
                    "Microstructure signal ({:?}, strength: {:.2}) conflicts with {} entry",
                    composite.direction,
                    composite.strength,
                    match signal.direction {
                        SignalDirection::Long => "long",
                        SignalDirection::Short => "short",
                        SignalDirection::Exit => "exit",
                    }
                ),
            })
        } else {
            None
        }
    }

    fn check_entry_timing(&self, signal: &SignalEvent, micro: &CachedMicroSignals) -> bool {
        let ob = &micro.order_book_imbalance;

        match (&signal.direction, ob.direction) {
            (SignalDirection::Long, Direction::Up) => {
                ob.strength >= self.config.timing_support_threshold
            }
            (SignalDirection::Short, Direction::Down) => {
                ob.strength >= self.config.timing_support_threshold
            }
            (SignalDirection::Long, Direction::Neutral)
            | (SignalDirection::Short, Direction::Neutral) => true,
            _ => false,
        }
    }

    fn apply_sizing_adjustment(
        &self,
        mut signal: SignalEvent,
        micro: &CachedMicroSignals,
    ) -> Option<SignalEvent> {
        if micro.is_high_stress() {
            signal.strength *= self.config.stress_size_multiplier;
            Some(signal)
        } else {
            None
        }
    }
}
```

### 4. EnhancedStrategy - Strategy Wrapper

```rust
// crates/signals/src/bridge/enhanced_strategy.rs

use algo_trade_core::traits::Strategy;
use algo_trade_core::events::{MarketEvent, SignalEvent};
use anyhow::Result;
use async_trait::async_trait;

/// Wraps a base strategy with microstructure filtering
pub struct EnhancedStrategy<S: Strategy> {
    inner: S,
    signals: SharedMicroSignals,
    filter: MicrostructureFilter,
    last_signal: Option<SignalEvent>,
}

impl<S: Strategy> EnhancedStrategy<S> {
    pub fn new(
        strategy: S,
        signals: SharedMicroSignals,
        config: MicrostructureFilterConfig,
    ) -> Self {
        Self {
            inner: strategy,
            signals,
            filter: MicrostructureFilter::new(config),
            last_signal: None,
        }
    }

    pub fn with_entry_filter(mut self, threshold: f64) -> Self {
        self.filter.config.entry_filter_enabled = true;
        self.filter.config.entry_filter_threshold = threshold;
        self
    }

    pub fn with_exit_triggers(
        mut self,
        liquidation_threshold: f64,
        funding_threshold: f64,
    ) -> Self {
        self.filter.config.exit_trigger_enabled = true;
        self.filter.config.exit_liquidation_threshold = liquidation_threshold;
        self.filter.config.exit_funding_threshold = funding_threshold;
        self
    }

    pub fn with_sizing_adjustment(mut self, stress_multiplier: f64) -> Self {
        self.filter.config.sizing_adjustment_enabled = true;
        self.filter.config.stress_size_multiplier = stress_multiplier;
        self
    }

    pub fn with_entry_timing(mut self, support_threshold: f64) -> Self {
        self.filter.config.entry_timing_enabled = true;
        self.filter.config.timing_support_threshold = support_threshold;
        self
    }
}

#[async_trait]
impl<S: Strategy> Strategy for EnhancedStrategy<S> {
    async fn on_market_event(&mut self, event: &MarketEvent) -> Result<Option<SignalEvent>> {
        let micro = {
            let guard = self.signals.read().await;
            guard.clone()
        };

        // Check forced exit based on microstructure
        if self.filter.config.exit_trigger_enabled {
            if let Some(ref last) = self.last_signal {
                if last.direction != SignalDirection::Exit {
                    let check_signal = SignalEvent {
                        direction: last.direction.clone(),
                        ..last.clone()
                    };

                    match self.filter.apply(check_signal, &micro) {
                        FilterResult::ForceExit { reason, signal } => {
                            tracing::info!(
                                "Microstructure forced exit: {} (symbol: {})",
                                reason,
                                signal.symbol
                            );
                            self.last_signal = Some(signal.clone());
                            return Ok(Some(signal));
                        }
                        _ => {}
                    }
                }
            }
        }

        let maybe_signal = self.inner.on_market_event(event).await?;

        let Some(signal) = maybe_signal else {
            return Ok(None);
        };

        match self.filter.apply(signal, &micro) {
            FilterResult::Allow(s) => {
                self.last_signal = Some(s.clone());
                Ok(Some(s))
            }
            FilterResult::Block { reason } => {
                tracing::debug!("Signal blocked by microstructure filter: {}", reason);
                Ok(None)
            }
            FilterResult::Modify(s) => {
                tracing::debug!(
                    "Signal modified by microstructure filter: strength adjusted"
                );
                self.last_signal = Some(s.clone());
                Ok(Some(s))
            }
            FilterResult::ForceExit { reason, signal } => {
                tracing::info!("Microstructure forced exit: {}", reason);
                self.last_signal = Some(signal.clone());
                Ok(Some(signal))
            }
        }
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }
}
```

---

## Usage Examples

### Example 1: MACrossover with Entry Filter

```rust
use algo_trade_signals::bridge::{
    CachedMicroSignals, EnhancedStrategy, MicrostructureFilterConfig,
    MicrostructureOrchestrator, SharedMicroSignals,
};
use algo_trade_strategy::MaCrossoverStrategy;
use std::sync::Arc;
use tokio::sync::RwLock;

async fn setup_enhanced_trading() -> Result<()> {
    let pool = PgPool::connect(&database_url).await?;

    // Create shared signal cache
    let signals: SharedMicroSignals = Arc::new(RwLock::new(CachedMicroSignals::default()));

    // Start background signal collection
    let orchestrator = MicrostructureOrchestrator::new(
        pool.clone(),
        "BTCUSDT",
        "hyperliquid",
        signals.clone(),
    );
    let _cmd_tx = orchestrator.spawn();

    // Wrap MA crossover with microstructure filtering
    let base_strategy = MaCrossoverStrategy::new("BTCUSDT".to_string(), 10, 30);

    let enhanced = EnhancedStrategy::new(
        base_strategy,
        signals,
        MicrostructureFilterConfig::default(),
    )
    .with_entry_filter(0.6)
    .with_exit_triggers(0.8, 0.9)
    .with_sizing_adjustment(0.5);

    // Use in trading system
    let strategy: Arc<Mutex<dyn Strategy>> = Arc::new(Mutex::new(enhanced));

    Ok(())
}
```

### Example 2: QuadMA with Entry Timing

```rust
let enhanced = EnhancedStrategy::new(
    QuadMaStrategy::new("ETHUSDT".to_string()),
    signals,
    MicrostructureFilterConfig {
        entry_filter_enabled: false,
        entry_timing_enabled: true,
        timing_support_threshold: 0.3,
        ..Default::default()
    },
);
```

### Example 3: Conservative Risk Profile

```rust
let config = MicrostructureFilterConfig {
    entry_filter_enabled: true,
    entry_filter_threshold: 0.4,
    exit_trigger_enabled: true,
    exit_liquidation_threshold: 0.6,
    exit_funding_threshold: 0.7,
    sizing_adjustment_enabled: true,
    stress_size_multiplier: 0.25,
    entry_timing_enabled: true,
    timing_support_threshold: 0.4,
};
```

---

## Integration Points

### 1. TradingSystem Integration

The `TradingSystem` in `crates/core/src/engine.rs` already accepts `Arc<Mutex<dyn Strategy>>`. The `EnhancedStrategy` implements `Strategy`, so it works seamlessly:

```rust
pub struct TradingSystem<D, E> {
    strategies: Vec<Arc<Mutex<dyn Strategy>>>,  // EnhancedStrategy fits here
}
```

### 2. CLI Integration

Add command-line flags for microstructure features:

```rust
#[derive(Parser)]
struct LiveArgs {
    #[arg(long, default_value = "false")]
    enable_micro_filter: bool,

    #[arg(long, default_value = "0.6")]
    filter_threshold: f64,

    #[arg(long, default_value = "false")]
    enable_entry_timing: bool,
}
```

### 3. Database Requirements

The `SignalContextBuilder` already handles database queries. Ensure these tables are populated:
- `orderbook_snapshots` - For imbalance signals
- `funding_rates` - For funding rate signals
- `liquidations` - For cascade signals
- `news_events` - For news signals (optional)

---

## Trade-offs Analysis

### Approach 1: Wrapper Pattern (Recommended)

**Pros**:
- No changes to existing trait signatures
- Clean separation of concerns
- Strategies can be used with or without filtering
- Easy to test each layer independently
- Gradual adoption possible

**Cons**:
- Additional wrapper overhead
- Two levels of async (orchestrator + strategy)
- Requires background task for signal updates

### Approach 2: Trait Extension

```rust
#[async_trait]
pub trait MicrostructureAwareStrategy: Strategy {
    fn set_micro_context(&mut self, signals: &CachedMicroSignals);
}
```

**Pros**:
- More direct integration
- No wrapper needed

**Cons**:
- Requires changes to existing strategies
- Tight coupling
- Breaks existing code

### Approach 3: Event-Based Bridge

```rust
pub enum MarketEvent {
    MicrostructureUpdate {
        signals: CachedMicroSignals,
        timestamp: DateTime<Utc>,
    },
}
```

**Pros**:
- Uses existing event system
- Strategies can react directly

**Cons**:
- Requires all strategies to handle new event type
- Pollutes MarketEvent with non-market data
- Mixing concerns

---

## File Structure

```
crates/signals/src/
├── bridge/
│   ├── mod.rs
│   ├── cached_signals.rs    # CachedMicroSignals, SharedMicroSignals
│   ├── orchestrator.rs      # MicrostructureOrchestrator
│   ├── filter.rs            # MicrostructureFilter, FilterResult
│   └── enhanced_strategy.rs # EnhancedStrategy<S>
├── generator/               # Existing signal generators
│   └── ...
└── lib.rs                   # Add `pub mod bridge;`
```

---

## Testing Strategy

### Unit Tests

1. **Filter logic**: Test each filter condition independently
2. **Direction mapping**: Ensure Up/Down/Neutral maps correctly to Long/Short/Exit
3. **Threshold behavior**: Verify signals blocked/allowed at boundary values
4. **Sizing calculation**: Check stress multiplier application

### Integration Tests

1. **End-to-end flow**: Strategy -> Filter -> Modified output
2. **Database integration**: SignalContextBuilder -> CachedMicroSignals
3. **Concurrent access**: Multiple readers during orchestrator write

### Property Tests

```rust
#[test]
fn filter_never_blocks_exit_signals() {
    proptest!(|(strength in 0.0..1.0)| {
        let signal = SignalEvent { direction: SignalDirection::Exit, .. };
        let result = filter.apply(signal.clone(), &micro);
        assert!(!matches!(result, FilterResult::Block { .. }));
    });
}
```

---

## Performance Considerations

1. **RwLock contention**: Readers do not block each other; writes are infrequent (every 5s)
2. **Database queries**: Batched in orchestrator, not per-strategy-event
3. **Memory**: CachedMicroSignals is small (~200 bytes), clone is cheap
4. **Latency**: Strategy adds one RwLock read (~nanoseconds)

---

## Migration Path

1. **Phase 1**: Implement core bridge types (CachedMicroSignals, Filter)
2. **Phase 2**: Add orchestrator with configurable update interval
3. **Phase 3**: Create EnhancedStrategy wrapper
4. **Phase 4**: Add CLI integration flags
5. **Phase 5**: Backtest comparison (with/without filtering)
6. **Phase 6**: Paper trade validation
7. **Phase 7**: Production deployment with conservative defaults

---

## Go/No-Go Criteria

Before production deployment:
- [ ] Unit test coverage > 80% for bridge module
- [ ] Integration test with live data feed
- [ ] Paper trade for 1000+ bars showing filter effectiveness
- [ ] Documented performance impact < 1ms per event
- [ ] Backtest showing improved risk-adjusted returns OR reduced drawdown

---

## Future Extensions

1. **Adaptive thresholds**: Adjust filter thresholds based on market volatility
2. **Multi-symbol correlation**: Use BTC microstructure to filter altcoin trades
3. **Machine learning integration**: Train classifier on filter outcomes
4. **Real-time alerts**: Webhook notifications when signals blocked
5. **Dashboard**: Visualize filter decisions in web UI
