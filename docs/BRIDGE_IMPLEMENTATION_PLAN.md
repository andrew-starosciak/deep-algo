# Signal-Strategy Bridge Implementation Plan

## HANDOFF: planner -> tdd-guide

### Context
Implementing the bridge module from `docs/SIGNAL_STRATEGY_BRIDGE.md` to enable microstructure signals to enhance traditional trading strategies.

### Implementation Order

**Phase 1: Core Types** (no dependencies)
1. `crates/signals/src/bridge/mod.rs` - Module structure
2. `crates/signals/src/bridge/cached_signals.rs` - CachedMicroSignals, SharedMicroSignals

**Phase 2: Filter Logic** (depends on Phase 1)
3. `crates/signals/src/bridge/filter.rs` - MicrostructureFilter, FilterResult, MicrostructureFilterConfig

**Phase 3: Orchestrator** (depends on Phase 1, existing generators)
4. `crates/signals/src/bridge/orchestrator.rs` - MicrostructureOrchestrator, OrchestratorCommand

**Phase 4: Strategy Wrapper** (depends on Phase 1, 2)
5. `crates/signals/src/bridge/enhanced_strategy.rs` - EnhancedStrategy<S>

**Phase 5: Integration**
6. Update `crates/signals/src/lib.rs` - Add `pub mod bridge;`

### Dependencies from Existing Code

```rust
// From algo_trade_core
use algo_trade_core::signal::{SignalValue, Direction, SignalContext};
use algo_trade_core::events::{SignalDirection, SignalEvent, MarketEvent};
use algo_trade_core::traits::Strategy;

// From algo_trade_signals
use crate::context_builder::SignalContextBuilder;
use crate::generator::{
    OrderBookImbalanceSignal,
    FundingRateSignal,
    LiquidationCascadeSignal,
    NewsSignal,
    CompositeSignal,
};
```

### Required Trait Implementations

1. **EnhancedStrategy<S>** must implement:
   - `Strategy` trait (async `on_market_event`, `name`)
   - `Send + Sync` bounds

2. **CachedMicroSignals** must implement:
   - `Clone` (cheap clone for read access)
   - `Default` (neutral values)
   - `Debug`

### Test Categories (TDD Order)

**Unit Tests:**
1. `cached_signals::tests` - Direction consensus, stress detection, direction support
2. `filter::tests` - Entry filter, exit trigger, sizing adjustment, entry timing
3. `enhanced_strategy::tests` - Signal passthrough, blocking, modification

**Integration Tests:**
4. `tests/bridge_integration.rs` - Full flow with MaCrossover

### Acceptance Criteria

- [ ] `cargo check -p algo-trade-signals` passes
- [ ] `cargo test -p algo-trade-signals` passes
- [ ] `cargo clippy -p algo-trade-signals -- -D warnings` passes
- [ ] Unit test coverage > 80% for bridge module
- [ ] EnhancedStrategy works with MaCrossoverStrategy
- [ ] Filter correctly blocks conflicting entries
- [ ] Exit triggers fire on extreme conditions

### Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| SignalValue::neutral() may not exist | Create helper or check core crate |
| Strategy trait may be sync | Check actual trait signature |
| Direction enum mismatch | Map carefully between Up/Down and Long/Short |

### Open Questions for TDD-guide

1. Does `SignalValue` have a `neutral()` constructor?
2. Is the `Strategy` trait async or sync?
3. Are there existing tests we can reference for patterns?
