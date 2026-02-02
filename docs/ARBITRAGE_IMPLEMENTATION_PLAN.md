# Arbitrage Implementation Plan
## Polymarket BTC 15-Minute Binary Markets

**Status**: Planning Phase
**Target**: Production-ready arbitrage execution with statistical validation
**Estimated Phases**: 6 phases over 4-6 weeks

---

## Executive Summary

This plan implements pure arbitrage for Polymarket BTC 15-minute binary markets, where buying both YES and NO when combined cost < $1.00 guarantees profit regardless of outcome.

### Key Metrics

| Metric | Target |
|--------|--------|
| Break-even threshold | Pair cost < $0.983 |
| Conservative threshold | Pair cost < $0.97 (3% margin) |
| Minimum fill rate (Wilson CI lower) | > 60% |
| Maximum imbalance | ±50 shares |
| Minimum sample size | 41 attempts for validation |

---

## Phase 1: Core Data Types & Order Book (Week 1)

### 1.1 Arbitrage Data Structures

**File**: `crates/exchange-polymarket/src/arbitrage/types.rs`

```rust
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// L2 order book with incremental update support
#[derive(Debug, Clone)]
pub struct L2OrderBook {
    pub token_id: String,
    pub bids: BTreeMap<Decimal, Decimal>,  // price -> size, sorted desc
    pub asks: BTreeMap<Decimal, Decimal>,  // price -> size, sorted asc
    pub last_update_ms: Option<i64>,
}

impl L2OrderBook {
    pub fn best_bid(&self) -> Option<Decimal> {
        self.bids.keys().next_back().copied()
    }

    pub fn best_ask(&self) -> Option<Decimal> {
        self.asks.keys().next().copied()
    }

    pub fn apply_snapshot(&mut self, bids: Vec<(Decimal, Decimal)>, asks: Vec<(Decimal, Decimal)>) {
        self.bids.clear();
        self.asks.clear();
        for (price, size) in bids {
            if size > Decimal::ZERO { self.bids.insert(price, size); }
        }
        for (price, size) in asks {
            if size > Decimal::ZERO { self.asks.insert(price, size); }
        }
    }

    pub fn apply_delta(&mut self, side: Side, price: Decimal, size: Decimal) {
        let book = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        if size <= Decimal::ZERO {
            book.remove(&price);
        } else {
            book.insert(price, size);
        }
    }
}

/// Result of walking the order book for a given size
#[derive(Debug, Clone)]
pub struct FillSimulation {
    pub filled: Decimal,
    pub total_cost: Decimal,
    pub vwap: Decimal,
    pub worst_price: Decimal,
    pub best_price: Decimal,
    pub sufficient_depth: bool,
}

/// Detected arbitrage opportunity
#[derive(Debug, Clone, Serialize)]
pub struct ArbitrageOpportunity {
    pub market_id: String,
    pub yes_token_id: String,
    pub no_token_id: String,

    // Prices
    pub yes_worst_fill: Decimal,
    pub no_worst_fill: Decimal,
    pub pair_cost: Decimal,

    // Profit analysis
    pub gross_profit_per_pair: Decimal,
    pub expected_fee: Decimal,
    pub net_profit_per_pair: Decimal,
    pub roi: Decimal,

    // Sizing
    pub recommended_size: Decimal,
    pub total_investment: Decimal,
    pub guaranteed_payout: Decimal,

    // Risk
    pub yes_depth: Decimal,
    pub no_depth: Decimal,
    pub risk_score: f64,

    pub detected_at: chrono::DateTime<chrono::Utc>,
}

/// Paired arbitrage position tracking
#[derive(Debug, Clone)]
pub struct ArbitragePosition {
    pub id: uuid::Uuid,
    pub market_id: String,

    // YES leg
    pub yes_shares: Decimal,
    pub yes_cost: Decimal,
    pub yes_avg_price: Decimal,

    // NO leg
    pub no_shares: Decimal,
    pub no_cost: Decimal,
    pub no_avg_price: Decimal,

    // Combined metrics
    pub pair_cost: Decimal,
    pub guaranteed_payout: Decimal,
    pub imbalance: Decimal,

    pub opened_at: chrono::DateTime<chrono::Utc>,
    pub status: PositionStatus,
}

impl ArbitragePosition {
    pub fn pair_cost(&self) -> Decimal {
        let min_qty = self.yes_shares.min(self.no_shares);
        if min_qty == Decimal::ZERO {
            return Decimal::MAX;
        }
        (self.yes_cost + self.no_cost) / min_qty
    }

    pub fn guaranteed_profit(&self) -> Decimal {
        self.guaranteed_payout() - (self.yes_cost + self.no_cost)
    }

    pub fn guaranteed_payout(&self) -> Decimal {
        self.yes_shares.min(self.no_shares)
    }

    pub fn imbalance(&self) -> Decimal {
        self.yes_shares - self.no_shares
    }

    pub fn imbalance_ratio(&self) -> Decimal {
        let max = self.yes_shares.max(self.no_shares);
        if max == Decimal::ZERO { return Decimal::ZERO; }
        self.imbalance().abs() / max
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionStatus {
    Building,   // Accumulating shares
    Complete,   // Balanced and within threshold
    Settling,   // Market closed, awaiting payout
    Settled,    // Final P&L realized
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    FOK,  // Fill-or-Kill (required for arbitrage)
    FAK,  // Fill-and-Kill (for unwinding)
    GTC,  // Good-til-Cancelled (not recommended)
}
```

### 1.2 Order Book Walking Algorithm

**File**: `crates/exchange-polymarket/src/arbitrage/orderbook.rs`

```rust
use rust_decimal::Decimal;
use super::types::{L2OrderBook, FillSimulation, Side};

/// Walk the order book to calculate actual fill cost for a given size
pub fn simulate_fill(
    book: &L2OrderBook,
    side: Side,
    target_size: Decimal,
) -> Option<FillSimulation> {
    if target_size <= Decimal::ZERO {
        return None;
    }

    let levels: Vec<(Decimal, Decimal)> = match side {
        Side::Buy => book.asks.iter().map(|(p, s)| (*p, *s)).collect(),
        Side::Sell => book.bids.iter().rev().map(|(p, s)| (*p, *s)).collect(),
    };

    if levels.is_empty() {
        return None;
    }

    let mut filled = Decimal::ZERO;
    let mut total_cost = Decimal::ZERO;
    let mut worst_price = Decimal::ZERO;
    let best_price = levels.first().map(|(p, _)| *p)?;

    for (price, size) in &levels {
        if filled >= target_size {
            break;
        }
        let take = (*size).min(target_size - filled);
        total_cost += take * price;
        filled += take;
        worst_price = *price;
    }

    let sufficient_depth = filled >= target_size;
    let vwap = if filled > Decimal::ZERO {
        total_cost / filled
    } else {
        Decimal::ZERO
    };

    Some(FillSimulation {
        filled,
        total_cost,
        vwap,
        worst_price,
        best_price,
        sufficient_depth,
    })
}
```

---

## Phase 2: Arbitrage Detection Engine (Week 1-2)

### 2.1 Opportunity Detector

**File**: `crates/exchange-polymarket/src/arbitrage/detector.rs`

```rust
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use super::types::*;
use super::orderbook::simulate_fill;

pub struct ArbitrageDetector {
    /// Maximum pair cost to consider (e.g., 0.97 for 3% margin)
    pub target_pair_cost: Decimal,
    /// Minimum net profit per pair after fees
    pub min_profit_threshold: Decimal,
    /// Maximum position size per opportunity
    pub max_position_size: Decimal,
    /// Gas cost per transaction (Polygon)
    pub gas_cost: Decimal,
}

impl Default for ArbitrageDetector {
    fn default() -> Self {
        Self {
            target_pair_cost: dec!(0.97),
            min_profit_threshold: dec!(0.005),  // 0.5 cents
            max_position_size: dec!(1000),
            gas_cost: dec!(0.007),
        }
    }
}

impl ArbitrageDetector {
    /// Detect arbitrage opportunity from YES and NO order books
    pub fn detect(
        &self,
        market_id: &str,
        yes_book: &L2OrderBook,
        no_book: &L2OrderBook,
        order_size: Decimal,
    ) -> Option<ArbitrageOpportunity> {
        // Simulate fills for both sides
        let yes_fill = simulate_fill(yes_book, Side::Buy, order_size)?;
        let no_fill = simulate_fill(no_book, Side::Buy, order_size)?;

        // Check sufficient depth
        if !yes_fill.sufficient_depth || !no_fill.sufficient_depth {
            return None;
        }

        // Calculate pair cost using WORST fill prices
        let pair_cost = yes_fill.worst_price + no_fill.worst_price;

        // Check threshold
        if pair_cost > self.target_pair_cost {
            return None;
        }

        // Calculate profits
        let gross_profit = Decimal::ONE - pair_cost;

        // Fee is 2% of PROFIT on winning side, expected value calculation
        // E[Fee] = 0.01 * (2 - pair_cost)
        let expected_fee = dec!(0.01) * (dec!(2) - pair_cost);

        // Gas for 2 transactions
        let total_gas = self.gas_cost * dec!(2);

        let net_profit = gross_profit - expected_fee - total_gas;

        // Check minimum profit threshold
        if net_profit < self.min_profit_threshold {
            return None;
        }

        // Calculate sizing
        let size = order_size.min(self.max_position_size);
        let total_investment = size * pair_cost;
        let guaranteed_payout = size;  // One side always pays $1

        // ROI calculation
        let roi = if total_investment > Decimal::ZERO {
            net_profit * size / total_investment * dec!(100)
        } else {
            Decimal::ZERO
        };

        // Risk scoring (0.0 = low risk, 1.0 = high risk)
        let risk_score = self.calculate_risk_score(&yes_fill, &no_fill, pair_cost);

        Some(ArbitrageOpportunity {
            market_id: market_id.to_string(),
            yes_token_id: yes_book.token_id.clone(),
            no_token_id: no_book.token_id.clone(),
            yes_worst_fill: yes_fill.worst_price,
            no_worst_fill: no_fill.worst_price,
            pair_cost,
            gross_profit_per_pair: gross_profit,
            expected_fee,
            net_profit_per_pair: net_profit,
            roi,
            recommended_size: size,
            total_investment,
            guaranteed_payout,
            yes_depth: yes_fill.filled,
            no_depth: no_fill.filled,
            risk_score,
            detected_at: chrono::Utc::now(),
        })
    }

    fn calculate_risk_score(
        &self,
        yes_fill: &FillSimulation,
        no_fill: &FillSimulation,
        pair_cost: Decimal,
    ) -> f64 {
        let mut risk = 0.0;

        // Slippage risk: difference between best and worst price
        let yes_slippage = (yes_fill.worst_price - yes_fill.best_price).abs();
        let no_slippage = (no_fill.worst_price - no_fill.best_price).abs();
        let total_slippage = yes_slippage + no_slippage;

        // High slippage = higher risk
        risk += (total_slippage.to_f64().unwrap_or(0.0) * 10.0).min(0.3);

        // Thin margin risk: closer to threshold = higher risk
        let margin = self.target_pair_cost - pair_cost;
        if margin < dec!(0.01) {
            risk += 0.3;
        } else if margin < dec!(0.02) {
            risk += 0.15;
        }

        // Depth imbalance risk
        let depth_ratio = if yes_fill.filled > no_fill.filled {
            no_fill.filled / yes_fill.filled
        } else {
            yes_fill.filled / no_fill.filled
        };
        if depth_ratio.to_f64().unwrap_or(1.0) < 0.5 {
            risk += 0.2;
        }

        risk.min(1.0)
    }
}
```

---

## Phase 3: Order Execution (Week 2-3) [P0 BLOCKER]

### 3.1 Polymarket CLOB Client Extension

**File**: `crates/exchange-polymarket/src/execution.rs`

This requires implementing:

1. **EIP-712 Signing**: For order authentication
2. **POST Endpoints**: For order submission
3. **Order Types**: FOK, FAK, GTC support
4. **Order Status Polling**: To verify fills

```rust
use async_trait::async_trait;
use rust_decimal::Decimal;
use crate::arbitrage::types::{OrderType, Side};

/// Order parameters for submission
#[derive(Debug, Clone)]
pub struct OrderParams {
    pub token_id: String,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub order_type: OrderType,
    pub neg_risk: bool,  // Required for BTC 15-min markets
}

/// Order result from submission
#[derive(Debug, Clone)]
pub struct OrderResult {
    pub order_id: String,
    pub status: OrderStatus,
    pub filled_size: Decimal,
    pub avg_fill_price: Option<Decimal>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    Pending,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
    Expired,
}

#[async_trait]
pub trait PolymarketExecutor: Send + Sync {
    /// Sign and submit a single order
    async fn submit_order(&self, order: OrderParams) -> Result<OrderResult, ExecutionError>;

    /// Pre-sign multiple orders for batch submission (faster)
    async fn submit_orders_batch(
        &self,
        orders: Vec<OrderParams>,
    ) -> Result<Vec<OrderResult>, ExecutionError>;

    /// Cancel an order by ID
    async fn cancel_order(&self, order_id: &str) -> Result<(), ExecutionError>;

    /// Get current order status
    async fn get_order_status(&self, order_id: &str) -> Result<OrderResult, ExecutionError>;

    /// Wait for order to reach terminal state
    async fn wait_for_terminal(
        &self,
        order_id: &str,
        timeout: std::time::Duration,
    ) -> Result<OrderResult, ExecutionError>;

    /// Get current positions
    async fn get_positions(&self) -> Result<Vec<Position>, ExecutionError>;

    /// Get available balance
    async fn get_balance(&self) -> Result<Decimal, ExecutionError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Order rejected: {reason}")]
    Rejected { reason: String },

    #[error("Insufficient balance: need {required}, have {available}")]
    InsufficientBalance { required: Decimal, available: Decimal },

    #[error("Timeout waiting for order: {order_id}")]
    Timeout { order_id: String },

    #[error("Partial fill: filled {filled} of {requested}")]
    PartialFill { order_id: String, filled: Decimal, requested: Decimal },

    #[error("API error: {0}")]
    Api(String),

    #[error("Signing error: {0}")]
    Signing(String),
}
```

### 3.2 Paired Execution Strategy

**File**: `crates/exchange-polymarket/src/arbitrage/executor.rs`

```rust
use super::types::*;
use crate::execution::{PolymarketExecutor, OrderParams, OrderResult, OrderStatus, ExecutionError};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::time::Duration;
use tracing::{info, warn, error};

pub struct ArbitrageExecutor<E: PolymarketExecutor> {
    executor: E,
    config: ExecutorConfig,
    state: ArbitrageState,
}

#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    pub balance_margin: Decimal,      // 1.20 for 20% safety margin
    pub cooldown: Duration,
    pub order_timeout: Duration,
    pub max_imbalance: Decimal,       // Maximum YES - NO difference
    pub max_daily_loss: Decimal,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            balance_margin: dec!(1.20),
            cooldown: Duration::from_secs(5),
            order_timeout: Duration::from_secs(3),
            max_imbalance: dec!(50),
            max_daily_loss: dec!(50),
        }
    }
}

#[derive(Debug, Default)]
struct ArbitrageState {
    last_execution: Option<std::time::Instant>,
    daily_pnl: Decimal,
    consecutive_failures: u32,
    positions: Vec<ArbitragePosition>,
}

#[derive(Debug)]
pub enum ExecutionResult {
    Success {
        position: ArbitragePosition,
        yes_order: OrderResult,
        no_order: OrderResult,
    },
    PartialFill {
        filled_side: Side,
        order: OrderResult,
        unwind_attempted: bool,
    },
    Rejected {
        reason: String,
    },
    RiskLimitHit {
        limit: String,
    },
}

impl<E: PolymarketExecutor> ArbitrageExecutor<E> {
    pub async fn execute(
        &mut self,
        opportunity: &ArbitrageOpportunity,
    ) -> Result<ExecutionResult, ExecutionError> {
        // Pre-execution risk checks
        self.check_risk_limits(opportunity)?;

        // Verify balance with margin
        let required = opportunity.total_investment * self.config.balance_margin;
        let available = self.executor.get_balance().await?;
        if available < required {
            return Err(ExecutionError::InsufficientBalance { required, available });
        }

        // Create orders
        let yes_order = OrderParams {
            token_id: opportunity.yes_token_id.clone(),
            side: Side::Buy,
            price: opportunity.yes_worst_fill,
            size: opportunity.recommended_size,
            order_type: OrderType::FOK,  // Critical: FOK prevents partial fills
            neg_risk: true,
        };

        let no_order = OrderParams {
            token_id: opportunity.no_token_id.clone(),
            side: Side::Buy,
            price: opportunity.no_worst_fill,
            size: opportunity.recommended_size,
            order_type: OrderType::FOK,
            neg_risk: true,
        };

        // Submit both orders in batch (pre-signed for speed)
        let results = self.executor.submit_orders_batch(vec![yes_order, no_order]).await?;

        // Wait for terminal states
        let yes_result = self.executor.wait_for_terminal(
            &results[0].order_id,
            self.config.order_timeout,
        ).await?;

        let no_result = self.executor.wait_for_terminal(
            &results[1].order_id,
            self.config.order_timeout,
        ).await?;

        // Analyze outcomes
        let yes_filled = yes_result.status == OrderStatus::Filled;
        let no_filled = no_result.status == OrderStatus::Filled;

        match (yes_filled, no_filled) {
            (true, true) => {
                // Success! Both legs filled
                let position = self.create_position(opportunity, &yes_result, &no_result);
                self.state.positions.push(position.clone());
                self.state.last_execution = Some(std::time::Instant::now());
                self.state.consecutive_failures = 0;

                info!(
                    market_id = %opportunity.market_id,
                    pair_cost = %position.pair_cost,
                    profit = %position.guaranteed_profit(),
                    "Arbitrage position opened"
                );

                Ok(ExecutionResult::Success {
                    position,
                    yes_order: yes_result,
                    no_order: no_result,
                })
            }

            (true, false) => {
                // Partial fill - YES only, need to unwind
                warn!("Partial fill: YES filled, NO rejected. Attempting unwind.");
                let unwind_result = self.attempt_unwind(
                    &opportunity.yes_token_id,
                    yes_result.filled_size,
                ).await;

                self.state.consecutive_failures += 1;

                Ok(ExecutionResult::PartialFill {
                    filled_side: Side::Buy,
                    order: yes_result,
                    unwind_attempted: unwind_result.is_ok(),
                })
            }

            (false, true) => {
                // Partial fill - NO only, need to unwind
                warn!("Partial fill: NO filled, YES rejected. Attempting unwind.");
                let unwind_result = self.attempt_unwind(
                    &opportunity.no_token_id,
                    no_result.filled_size,
                ).await;

                self.state.consecutive_failures += 1;

                Ok(ExecutionResult::PartialFill {
                    filled_side: Side::Sell,
                    order: no_result,
                    unwind_attempted: unwind_result.is_ok(),
                })
            }

            (false, false) => {
                // Both rejected - no action needed
                self.state.consecutive_failures += 1;

                Ok(ExecutionResult::Rejected {
                    reason: "Both orders rejected or expired".to_string(),
                })
            }
        }
    }

    async fn attempt_unwind(
        &self,
        token_id: &str,
        size: Decimal,
    ) -> Result<OrderResult, ExecutionError> {
        // Get current best bid
        // Submit FAK sell order at best bid
        let unwind_order = OrderParams {
            token_id: token_id.to_string(),
            side: Side::Sell,
            price: dec!(0.01),  // Low price to ensure fill
            size,
            order_type: OrderType::FAK,  // Fill what we can
            neg_risk: true,
        };

        self.executor.submit_order(unwind_order).await
    }

    fn check_risk_limits(&self, opportunity: &ArbitrageOpportunity) -> Result<(), ExecutionError> {
        // Cooldown check
        if let Some(last) = self.state.last_execution {
            if last.elapsed() < self.config.cooldown {
                return Err(ExecutionError::Rejected {
                    reason: "Cooldown active".to_string(),
                });
            }
        }

        // Daily loss limit
        if self.state.daily_pnl < -self.config.max_daily_loss {
            return Err(ExecutionError::Rejected {
                reason: format!("Daily loss limit hit: {}", self.state.daily_pnl),
            });
        }

        // Consecutive failure cooldown
        if self.state.consecutive_failures >= 3 {
            return Err(ExecutionError::Rejected {
                reason: "Too many consecutive failures".to_string(),
            });
        }

        Ok(())
    }

    fn create_position(
        &self,
        opportunity: &ArbitrageOpportunity,
        yes_result: &OrderResult,
        no_result: &OrderResult,
    ) -> ArbitragePosition {
        ArbitragePosition {
            id: uuid::Uuid::new_v4(),
            market_id: opportunity.market_id.clone(),
            yes_shares: yes_result.filled_size,
            yes_cost: yes_result.filled_size * yes_result.avg_fill_price.unwrap_or(opportunity.yes_worst_fill),
            yes_avg_price: yes_result.avg_fill_price.unwrap_or(opportunity.yes_worst_fill),
            no_shares: no_result.filled_size,
            no_cost: no_result.filled_size * no_result.avg_fill_price.unwrap_or(opportunity.no_worst_fill),
            no_avg_price: no_result.avg_fill_price.unwrap_or(opportunity.no_worst_fill),
            pair_cost: opportunity.pair_cost,
            guaranteed_payout: yes_result.filled_size.min(no_result.filled_size),
            imbalance: yes_result.filled_size - no_result.filled_size,
            opened_at: chrono::Utc::now(),
            status: PositionStatus::Complete,
        }
    }
}
```

---

## Phase 4: WebSocket Order Book (Week 3)

### 4.1 WebSocket Client

**File**: `crates/exchange-polymarket/src/websocket.rs`

```rust
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures::{StreamExt, SinkExt};
use tokio::sync::mpsc;
use crate::arbitrage::types::{L2OrderBook, Side};
use rust_decimal::Decimal;

const WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

pub struct PolymarketWebSocket {
    books: std::collections::HashMap<String, L2OrderBook>,
    event_tx: mpsc::Sender<BookEvent>,
}

#[derive(Debug, Clone)]
pub enum BookEvent {
    Snapshot { token_id: String, book: L2OrderBook },
    Delta { token_id: String, side: Side, price: Decimal, size: Decimal },
    Trade { token_id: String, price: Decimal, size: Decimal, side: Side },
}

impl PolymarketWebSocket {
    pub async fn connect(
        token_ids: Vec<String>,
    ) -> Result<(Self, mpsc::Receiver<BookEvent>), Box<dyn std::error::Error>> {
        let (event_tx, event_rx) = mpsc::channel(1000);

        let ws = Self {
            books: std::collections::HashMap::new(),
            event_tx,
        };

        // Spawn connection handler
        let tx = ws.event_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = Self::run_connection(token_ids, tx).await {
                tracing::error!("WebSocket error: {}", e);
            }
        });

        Ok((ws, event_rx))
    }

    async fn run_connection(
        token_ids: Vec<String>,
        tx: mpsc::Sender<BookEvent>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (ws_stream, _) = connect_async(WS_URL).await?;
        let (mut write, mut read) = ws_stream.split();

        // Subscribe to assets
        let sub_msg = serde_json::json!({
            "type": "MARKET",
            "assets_ids": token_ids,
        });
        write.send(Message::Text(sub_msg.to_string())).await?;

        // Process messages
        while let Some(msg) = read.next().await {
            match msg? {
                Message::Text(text) => {
                    if let Ok(events) = Self::parse_message(&text) {
                        for event in events {
                            let _ = tx.send(event).await;
                        }
                    }
                }
                Message::Ping(data) => {
                    write.send(Message::Pong(data)).await?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn parse_message(text: &str) -> Result<Vec<BookEvent>, serde_json::Error> {
        // Parse Polymarket WebSocket message format
        // Returns list of book events
        todo!("Implement message parsing based on Polymarket WebSocket format")
    }

    pub fn get_book(&self, token_id: &str) -> Option<&L2OrderBook> {
        self.books.get(token_id)
    }
}
```

---

## Phase 5: Statistical Validation (Week 4)

### 5.1 Arbitrage Metrics

**File**: `crates/exchange-polymarket/src/arbitrage/metrics.rs`

```rust
use rust_decimal::Decimal;

/// Metrics specific to arbitrage strategies
#[derive(Debug, Clone, Default)]
pub struct ArbitrageMetrics {
    // Opportunity Detection
    pub windows_analyzed: u32,
    pub opportunities_detected: u32,
    pub detection_rate: f64,
    pub detection_rate_wilson_ci: (f64, f64),

    // Execution Success
    pub attempts: u32,
    pub successful_pairs: u32,
    pub partial_fills: u32,
    pub fill_rate: f64,
    pub fill_rate_wilson_ci: (f64, f64),

    // Profit Distribution
    pub mean_net_profit_per_pair: Decimal,
    pub std_dev_profit: Decimal,
    pub profit_t_statistic: f64,
    pub profit_p_value: f64,

    // Imbalance Risk
    pub mean_imbalance: Decimal,
    pub max_imbalance: Decimal,
    pub imbalance_variance: Decimal,

    // Timing
    pub mean_opportunity_duration_ms: f64,
    pub mean_fill_latency_ms: f64,

    // Totals
    pub total_invested: Decimal,
    pub total_payout: Decimal,
    pub total_fees: Decimal,
    pub total_pnl: Decimal,
}

impl ArbitrageMetrics {
    /// Calculate Wilson score confidence interval for fill rate
    pub fn update_fill_rate_ci(&mut self) {
        let (lower, upper) = wilson_ci(self.successful_pairs, self.attempts, 1.96);
        self.fill_rate = if self.attempts > 0 {
            self.successful_pairs as f64 / self.attempts as f64
        } else {
            0.0
        };
        self.fill_rate_wilson_ci = (lower, upper);
    }

    /// Check if fill rate meets minimum threshold for production
    pub fn fill_rate_acceptable(&self) -> bool {
        // Require lower CI bound > 60%
        self.fill_rate_wilson_ci.0 > 0.60 && self.attempts >= 41
    }

    /// Check if profit is statistically significant
    pub fn profit_significant(&self) -> bool {
        self.profit_p_value < 0.10 && self.attempts >= 41
    }
}

/// Wilson score confidence interval
pub fn wilson_ci(successes: u32, total: u32, z: f64) -> (f64, f64) {
    if total == 0 {
        return (0.0, 0.0);
    }

    let n = total as f64;
    let p = successes as f64 / n;
    let z2 = z * z;

    let denom = 1.0 + z2 / n;
    let center = p + z2 / (2.0 * n);
    let spread = z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt();

    let lower = ((center - spread) / denom).max(0.0);
    let upper = ((center + spread) / denom).min(1.0);

    (lower, upper)
}

/// One-sample t-test for mean profit
pub fn profit_t_test(profits: &[Decimal]) -> (f64, f64) {
    if profits.len() < 2 {
        return (0.0, 1.0);
    }

    let n = profits.len() as f64;
    let mean: f64 = profits.iter()
        .map(|d| d.to_f64().unwrap_or(0.0))
        .sum::<f64>() / n;

    let variance: f64 = profits.iter()
        .map(|d| {
            let x = d.to_f64().unwrap_or(0.0);
            (x - mean).powi(2)
        })
        .sum::<f64>() / (n - 1.0);

    let std_err = (variance / n).sqrt();

    if std_err == 0.0 {
        return (f64::INFINITY, 0.0);
    }

    let t_stat = mean / std_err;

    // Approximate p-value using normal distribution for large n
    let p_value = 2.0 * (1.0 - normal_cdf(t_stat.abs()));

    (t_stat, p_value)
}

fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + libm::erf(x / std::f64::consts::SQRT_2))
}
```

---

## Phase 6: CLI Integration (Week 4-5)

### 6.1 Arbitrage Bot Command

**File**: `crates/cli/src/commands/arbitrage_bot.rs`

```rust
use clap::Args;
use rust_decimal::Decimal;
use std::time::Duration;

#[derive(Args, Debug)]
pub struct ArbitrageBotArgs {
    /// Trading mode: paper or live
    #[arg(long, default_value = "paper")]
    pub mode: TradingMode,

    /// Target pair cost threshold (e.g., 0.97)
    #[arg(long, default_value = "0.97")]
    pub threshold: Decimal,

    /// Order size per opportunity
    #[arg(long, default_value = "100")]
    pub order_size: Decimal,

    /// Maximum position size per market
    #[arg(long, default_value = "1000")]
    pub max_position: Decimal,

    /// Maximum daily loss before stopping
    #[arg(long, default_value = "50")]
    pub max_daily_loss: Decimal,

    /// Cooldown between executions (seconds)
    #[arg(long, default_value = "5")]
    pub cooldown_secs: u64,

    /// Use WebSocket for order book (faster)
    #[arg(long)]
    pub use_websocket: bool,

    /// Polling interval for REST mode (milliseconds)
    #[arg(long, default_value = "500")]
    pub poll_interval_ms: u64,

    /// Duration to run (e.g., "2h", "1d")
    #[arg(long)]
    pub duration: Option<String>,

    /// Dry run (log trades without executing)
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum TradingMode {
    Paper,
    Live,
}
```

---

## Go/No-Go Criteria

### Phase 2 Gate: Detection Validation
- [ ] Detected 10+ opportunities in 24h paper monitoring
- [ ] Pair cost calculations match manual verification
- [ ] Order book walk algorithm tested with real data

### Phase 3 Gate: Execution Readiness
- [ ] FOK orders work on testnet
- [ ] Partial fill detection and unwind works
- [ ] Balance verification accurate

### Phase 5 Gate: Statistical Validation
- [ ] 41+ paper trade attempts completed
- [ ] Fill rate Wilson CI lower bound > 60%
- [ ] Mean profit t-test p-value < 0.10
- [ ] No imbalance events > 50 shares

### Phase 6 Gate: Production Readiness
- [ ] 200+ paper trades with positive EV
- [ ] 7+ days of stable operation
- [ ] Kill switch tested
- [ ] Monitoring dashboard operational

---

## Dependencies to Add

```toml
# crates/exchange-polymarket/Cargo.toml
[dependencies]
tokio-tungstenite = "0.21"
ethers = { version = "2.0", features = ["signing"] }
uuid = { version = "1", features = ["v4", "serde"] }
libm = "0.2"  # For erf function in statistics
```

---

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| One-leg fills | FOK orders + automatic unwind |
| Insufficient depth | Order book walk before execution |
| Balance issues | 20% safety margin + pre-check |
| Stale prices | WebSocket + freshness check |
| Exchange outage | Graceful degradation + alerts |
| Daily loss | Hard limit with auto-stop |
| Imbalance exposure | Max 50 shares imbalance limit |

---

## Timeline Summary

| Week | Phase | Deliverable |
|------|-------|-------------|
| 1 | Core Types + Detection | Data structures, order book walk, opportunity detection |
| 2 | Execution Foundation | Order types, signing, batch submission |
| 3 | Full Execution | Paired execution, partial fill handling, risk controls |
| 3-4 | WebSocket | Real-time order book streaming |
| 4 | Statistics | Metrics, validation, Go/No-Go gates |
| 4-5 | CLI Integration | Arbitrage bot command, paper trading |
| 5-6 | Production Prep | Live mode, monitoring, documentation |

---

## References

- [Polymarket CLOB API Docs](https://docs.polymarket.com/)
- [IMDEA Arbitrage Research Paper](https://arxiv.org/abs/2508.03474)
- [gabagool Implementation](https://github.com/gabagool222/15min-btc-polymarket-trading-bot)
- [PMXT Unified API](https://github.com/pmxt-dev/pmxt)
