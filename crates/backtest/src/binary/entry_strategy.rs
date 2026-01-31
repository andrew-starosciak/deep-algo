//! Entry strategy simulation for binary outcome bets.
//!
//! This module provides types and utilities for simulating different entry timing
//! strategies within a betting window. Entry timing can significantly impact expected
//! value due to price movements during the window.
//!
//! # Strategies
//!
//! - `FixedTimeEntry`: Enter at a fixed offset from window open
//! - `EdgeThresholdEntry`: Enter when edge exceeds a threshold
//!
//! # Price Simulation
//!
//! The module uses Brownian bridge price paths to simulate realistic price movements
//! that are anchored at both the open and close prices.

use chrono::Duration;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use super::outcome::BetDirection;

// ============================================================
// Constants
// ============================================================

/// Minimum valid price for binary markets (1 cent / 1%).
const MIN_BINARY_PRICE: f64 = 0.01;
/// Maximum valid price for binary markets (99 cents / 99%).
const MAX_BINARY_PRICE: f64 = 0.99;

// ============================================================
// Core Types
// ============================================================

/// The decision made by an entry strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryDecision {
    /// Enter a position at the specified time offset from window open.
    Enter {
        /// Time offset from window open.
        offset: Duration,
        /// Direction of the bet.
        direction: BetDirection,
    },
    /// Do not enter a position (edge insufficient, cutoff reached, etc.).
    NoEntry {
        /// Reason for not entering.
        reason: String,
    },
}

impl EntryDecision {
    /// Creates an Enter decision.
    #[must_use]
    pub fn enter(offset: Duration, direction: BetDirection) -> Self {
        Self::Enter { offset, direction }
    }

    /// Creates a NoEntry decision with the given reason.
    #[must_use]
    pub fn no_entry(reason: impl Into<String>) -> Self {
        Self::NoEntry {
            reason: reason.into(),
        }
    }

    /// Returns true if this is an entry decision.
    #[must_use]
    pub fn is_entry(&self) -> bool {
        matches!(self, Self::Enter { .. })
    }
}

/// Context provided to entry strategies for decision making.
#[derive(Debug, Clone)]
pub struct EntryContext {
    /// Current price of the Yes outcome (0.0 to 1.0).
    pub current_price: Decimal,
    /// Estimated probability of Yes outcome (0.0 to 1.0).
    pub estimated_probability: Decimal,
    /// Current time offset from window open.
    pub current_offset: Duration,
    /// Total duration of the betting window.
    pub window_duration: Duration,
    /// Direction we would bet if entering.
    pub direction: BetDirection,
    /// Fee rate for the trade.
    pub fee_rate: Decimal,
}

impl EntryContext {
    /// Creates a new entry context.
    #[must_use]
    pub fn new(
        current_price: Decimal,
        estimated_probability: Decimal,
        current_offset: Duration,
        window_duration: Duration,
        direction: BetDirection,
        fee_rate: Decimal,
    ) -> Self {
        Self {
            current_price,
            estimated_probability,
            current_offset,
            window_duration,
            direction,
            fee_rate,
        }
    }

    /// Calculates the edge (expected value) for the given direction.
    ///
    /// For Yes bets: edge = P(yes) * (1 - price) - P(no) * price - fees
    /// For No bets: edge = P(no) * (1 - price) - P(yes) * price - fees
    ///
    /// Note: The "price" for a No bet is (1 - yes_price).
    #[must_use]
    pub fn calculate_edge(&self) -> Decimal {
        let (p_win, price) = match self.direction {
            BetDirection::Yes => (self.estimated_probability, self.current_price),
            BetDirection::No => (
                Decimal::ONE - self.estimated_probability,
                Decimal::ONE - self.current_price,
            ),
        };

        // EV = p_win * (1 - price) - (1 - p_win) * price - fees
        // Simplified: EV = p_win - price - fees
        let gross_edge = p_win - price;
        // Fee is based on the effective price for the direction we're betting
        let fee_cost = self.fee_rate * price;
        gross_edge - fee_cost
    }

    /// Returns the remaining time in the window.
    #[must_use]
    pub fn remaining_time(&self) -> Duration {
        self.window_duration - self.current_offset
    }

    /// Returns true if we are past the midpoint of the window.
    #[must_use]
    pub fn past_midpoint(&self) -> bool {
        self.current_offset > self.window_duration / 2
    }
}

// ============================================================
// Entry Strategy Trait
// ============================================================

/// A strategy for determining when to enter a binary bet.
///
/// Entry strategies evaluate the current market conditions and decide
/// whether to enter, wait, or skip entirely.
pub trait EntryStrategy: Send + Sync {
    /// Evaluates the current context and returns an entry decision.
    fn evaluate(&self, ctx: &EntryContext) -> EntryDecision;

    /// Returns the name of this strategy.
    fn name(&self) -> &str;

    /// Returns a description of the strategy.
    fn description(&self) -> &str {
        ""
    }
}

// ============================================================
// Fixed Time Entry Strategy
// ============================================================

/// Enters at a fixed time offset from window open.
///
/// This is the simplest entry strategy - it always enters at a
/// predetermined time regardless of current market conditions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixedTimeEntry {
    /// Time offset from window open to enter.
    pub entry_offset: Duration,
    /// Maximum time offset before giving up (cutoff).
    pub cutoff: Duration,
}

impl FixedTimeEntry {
    /// Creates a new fixed time entry strategy.
    #[must_use]
    pub fn new(entry_offset: Duration, cutoff: Duration) -> Self {
        Self {
            entry_offset,
            cutoff,
        }
    }

    /// Creates a strategy that enters immediately at window open.
    #[must_use]
    pub fn at_open() -> Self {
        Self::new(Duration::zero(), Duration::minutes(15))
    }

    /// Creates a strategy that enters at the midpoint of a 15-minute window.
    #[must_use]
    pub fn at_midpoint() -> Self {
        Self::new(Duration::minutes(7), Duration::minutes(14))
    }
}

impl EntryStrategy for FixedTimeEntry {
    fn evaluate(&self, ctx: &EntryContext) -> EntryDecision {
        // Check if we've passed the cutoff
        if ctx.current_offset >= self.cutoff {
            return EntryDecision::no_entry("Past cutoff time");
        }

        // Check if we've reached our entry time
        if ctx.current_offset >= self.entry_offset {
            return EntryDecision::enter(ctx.current_offset, ctx.direction);
        }

        // Not yet at entry time
        EntryDecision::no_entry("Before entry time")
    }

    fn name(&self) -> &str {
        "FixedTimeEntry"
    }

    fn description(&self) -> &str {
        "Enters at a fixed time offset from window open"
    }
}

// ============================================================
// Edge Threshold Entry Strategy
// ============================================================

/// Enters when the edge exceeds a minimum threshold.
///
/// This strategy waits for favorable pricing before entering,
/// potentially getting better odds at the cost of sometimes missing windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeThresholdEntry {
    /// Minimum edge required to enter.
    pub min_edge: Decimal,
    /// Maximum time offset before giving up (cutoff).
    pub cutoff: Duration,
}

impl EdgeThresholdEntry {
    /// Creates a new edge threshold entry strategy.
    #[must_use]
    pub fn new(min_edge: Decimal, cutoff: Duration) -> Self {
        Self { min_edge, cutoff }
    }

    /// Creates a strategy with a 5% minimum edge requirement.
    #[must_use]
    pub fn with_five_percent_edge() -> Self {
        Self::new(dec!(0.05), Duration::minutes(14))
    }

    /// Creates a strategy with a 3% minimum edge requirement.
    #[must_use]
    pub fn with_three_percent_edge() -> Self {
        Self::new(dec!(0.03), Duration::minutes(14))
    }
}

impl EntryStrategy for EdgeThresholdEntry {
    fn evaluate(&self, ctx: &EntryContext) -> EntryDecision {
        // Check if we've passed the cutoff
        if ctx.current_offset >= self.cutoff {
            return EntryDecision::no_entry("Past cutoff time");
        }

        // Calculate current edge
        let edge = ctx.calculate_edge();

        // Check if edge meets threshold
        if edge >= self.min_edge {
            return EntryDecision::enter(ctx.current_offset, ctx.direction);
        }

        EntryDecision::no_entry(format!(
            "Edge {:.4} below threshold {:.4}",
            edge, self.min_edge
        ))
    }

    fn name(&self) -> &str {
        "EdgeThresholdEntry"
    }

    fn description(&self) -> &str {
        "Enters when edge exceeds minimum threshold"
    }
}

// ============================================================
// Price Path Types
// ============================================================

/// A single price point in a price path.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PricePoint {
    /// Time offset from path start.
    pub offset: Duration,
    /// Price at this time.
    pub price: Decimal,
}

impl PricePoint {
    /// Creates a new price point.
    #[must_use]
    pub fn new(offset: Duration, price: Decimal) -> Self {
        Self { offset, price }
    }
}

/// A complete price path over a time window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricePath {
    /// Price points in chronological order.
    pub points: Vec<PricePoint>,
    /// Opening price (price at offset 0).
    pub open_price: Decimal,
    /// Closing price (price at final offset).
    pub close_price: Decimal,
    /// Total duration of the path.
    pub duration: Duration,
}

impl PricePath {
    /// Creates a new price path from a list of points.
    ///
    /// The points should be in chronological order.
    #[must_use]
    pub fn new(points: Vec<PricePoint>) -> Self {
        let open_price = points.first().map(|p| p.price).unwrap_or(Decimal::ZERO);
        let close_price = points.last().map(|p| p.price).unwrap_or(Decimal::ZERO);
        let duration = points.last().map(|p| p.offset).unwrap_or(Duration::zero());

        Self {
            points,
            open_price,
            close_price,
            duration,
        }
    }

    /// Creates a constant price path (no movement).
    #[must_use]
    pub fn constant(price: Decimal, duration: Duration, n_points: usize) -> Self {
        let step = if n_points > 1 {
            duration / (n_points - 1) as i32
        } else {
            duration
        };

        let points: Vec<PricePoint> = (0..n_points)
            .map(|i| PricePoint::new(step * i as i32, price))
            .collect();

        Self {
            points,
            open_price: price,
            close_price: price,
            duration,
        }
    }

    /// Returns the price at the given offset, interpolating if necessary.
    #[must_use]
    pub fn price_at_offset(&self, offset: Duration) -> Option<Decimal> {
        if self.points.is_empty() {
            return None;
        }

        // Handle boundary cases
        if offset <= Duration::zero() {
            return Some(self.open_price);
        }
        if offset >= self.duration {
            return Some(self.close_price);
        }

        // Find surrounding points for interpolation
        let mut prev: Option<&PricePoint> = None;
        for point in &self.points {
            if point.offset == offset {
                return Some(point.price);
            }
            if point.offset > offset {
                // Interpolate between prev and point
                if let Some(p) = prev {
                    let t_range = (point.offset - p.offset).num_milliseconds() as f64;
                    let t_offset = (offset - p.offset).num_milliseconds() as f64;
                    let ratio = if t_range > 0.0 {
                        Decimal::try_from(t_offset / t_range).unwrap_or(Decimal::ZERO)
                    } else {
                        Decimal::ZERO
                    };
                    return Some(p.price + ratio * (point.price - p.price));
                }
            }
            prev = Some(point);
        }

        // Offset is after last point
        Some(self.close_price)
    }

    /// Returns the minimum price in the path.
    #[must_use]
    pub fn min_price(&self) -> Decimal {
        self.points
            .iter()
            .map(|p| p.price)
            .min()
            .unwrap_or(Decimal::ZERO)
    }

    /// Returns the maximum price in the path.
    #[must_use]
    pub fn max_price(&self) -> Decimal {
        self.points
            .iter()
            .map(|p| p.price)
            .max()
            .unwrap_or(Decimal::ZERO)
    }

    /// Returns the price range (max - min).
    #[must_use]
    pub fn price_range(&self) -> Decimal {
        self.max_price() - self.min_price()
    }
}

// ============================================================
// Price Path Generator
// ============================================================

/// Configuration for price path generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricePathConfig {
    /// Number of price points to generate.
    pub n_points: usize,
    /// Volatility parameter (standard deviation of increments).
    pub volatility: f64,
    /// Optional seed for reproducible paths.
    pub seed: Option<u64>,
}

impl Default for PricePathConfig {
    fn default() -> Self {
        Self {
            n_points: 100,
            volatility: 0.02,
            seed: None,
        }
    }
}

impl PricePathConfig {
    /// Creates a new configuration.
    #[must_use]
    pub fn new(n_points: usize, volatility: f64) -> Self {
        Self {
            n_points,
            volatility,
            seed: None,
        }
    }

    /// Sets a seed for reproducible generation.
    #[must_use]
    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }
}

/// Generates realistic price paths using Brownian bridge.
///
/// A Brownian bridge is a continuous-time stochastic process that is
/// conditioned to start at a given value and end at another given value.
/// This is useful for simulating price paths within a trading window where
/// we know both the open and close prices.
pub struct PricePathGenerator {
    config: PricePathConfig,
}

impl PricePathGenerator {
    /// Creates a new generator with the given configuration.
    #[must_use]
    pub fn new(config: PricePathConfig) -> Self {
        Self { config }
    }

    /// Creates a generator with default configuration.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(PricePathConfig::default())
    }

    /// Returns a reference to the configuration.
    #[must_use]
    pub fn config(&self) -> &PricePathConfig {
        &self.config
    }

    /// Generates a Brownian bridge price path.
    ///
    /// The path starts at `open_price` and ends at `close_price`, with
    /// random fluctuations in between controlled by the volatility parameter.
    ///
    /// # Arguments
    ///
    /// * `open_price` - Starting price
    /// * `close_price` - Ending price
    /// * `duration` - Total duration of the path
    ///
    /// # Returns
    ///
    /// A `PricePath` with the specified endpoints and intermediate points.
    #[must_use]
    pub fn generate_brownian_bridge(
        &self,
        open_price: Decimal,
        close_price: Decimal,
        duration: Duration,
    ) -> PricePath {
        let n = self.config.n_points;
        if n < 2 {
            return PricePath::constant(open_price, duration, 1);
        }

        let mut rng = match self.config.seed {
            Some(seed) => ChaCha8Rng::seed_from_u64(seed),
            None => ChaCha8Rng::from_entropy(),
        };

        let open_f64 = f64::try_from(open_price).unwrap_or(0.5);
        let close_f64 = f64::try_from(close_price).unwrap_or(0.5);
        let dt = 1.0 / (n - 1) as f64;
        let vol = self.config.volatility;

        // Generate standard Brownian motion
        let mut w = vec![0.0; n];
        for i in 1..n {
            let z: f64 = self.standard_normal(&mut rng);
            w[i] = w[i - 1] + z * dt.sqrt();
        }

        // Convert to Brownian bridge: B(t) = W(t) - t * W(1)
        // Then scale and shift to match endpoints
        let mut prices = Vec::with_capacity(n);
        let step = duration / (n - 1) as i32;

        for i in 0..n {
            let t = i as f64 / (n - 1) as f64;

            // Brownian bridge component
            let bridge = w[i] - t * w[n - 1];

            // Linear interpolation between open and close
            let linear = open_f64 + t * (close_f64 - open_f64);

            // Add scaled bridge noise
            let price_f64 = linear + vol * bridge;

            // Clamp to valid range [0.01, 0.99]
            let price_clamped = price_f64.clamp(MIN_BINARY_PRICE, MAX_BINARY_PRICE);

            let price = Decimal::try_from(price_clamped).unwrap_or(dec!(0.50));
            let offset = step * i as i32;
            prices.push(PricePoint::new(offset, price));
        }

        // Ensure exact endpoints
        if let Some(first) = prices.first_mut() {
            first.price = open_price;
        }
        if let Some(last) = prices.last_mut() {
            last.price = close_price;
        }

        PricePath::new(prices)
    }

    /// Generates a standard normal random variable using Box-Muller transform.
    fn standard_normal(&self, rng: &mut ChaCha8Rng) -> f64 {
        let u1: f64 = rng.gen::<f64>().max(1e-10);
        let u2: f64 = rng.gen();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }

    /// Generates multiple price paths for Monte Carlo analysis.
    #[must_use]
    pub fn generate_paths(
        &self,
        open_price: Decimal,
        close_price: Decimal,
        duration: Duration,
        n_paths: usize,
    ) -> Vec<PricePath> {
        let mut paths = Vec::with_capacity(n_paths);
        let base_seed = self.config.seed.unwrap_or(0);

        for i in 0..n_paths {
            let config = PricePathConfig {
                n_points: self.config.n_points,
                volatility: self.config.volatility,
                seed: Some(base_seed.wrapping_add(i as u64)),
            };
            let generator = PricePathGenerator::new(config);
            paths.push(generator.generate_brownian_bridge(open_price, close_price, duration));
        }

        paths
    }
}

// ============================================================
// Entry Strategy Simulator
// ============================================================

/// Results from simulating an entry strategy over a price path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrySimulationResult {
    /// The strategy name.
    pub strategy_name: String,
    /// The decision made.
    pub decision: EntryDecision,
    /// The entry price (if entered).
    pub entry_price: Option<Decimal>,
    /// Edge at entry time (if entered).
    pub edge_at_entry: Option<Decimal>,
    /// Opening price of the path.
    pub open_price: Decimal,
    /// Closing price of the path.
    pub close_price: Decimal,
}

impl EntrySimulationResult {
    /// Returns true if entry was made.
    #[must_use]
    pub fn did_enter(&self) -> bool {
        self.decision.is_entry()
    }

    /// Returns the price improvement vs opening (positive = better).
    ///
    /// For Yes bets, lower entry price is better.
    /// For No bets, higher entry price (lower No price) is better.
    #[must_use]
    pub fn price_improvement(&self, direction: BetDirection) -> Option<Decimal> {
        let entry = self.entry_price?;
        match direction {
            BetDirection::Yes => Some(self.open_price - entry),
            BetDirection::No => Some(entry - self.open_price),
        }
    }
}

/// Aggregate statistics from multiple simulations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryStrategyStats {
    /// Strategy name.
    pub strategy_name: String,
    /// Number of simulations.
    pub n_simulations: usize,
    /// Number of times entry was made.
    pub n_entries: usize,
    /// Entry rate (n_entries / n_simulations).
    pub entry_rate: f64,
    /// Average entry price when entered.
    pub avg_entry_price: Option<Decimal>,
    /// Average edge at entry.
    pub avg_edge_at_entry: Option<Decimal>,
    /// Average price improvement vs open.
    pub avg_price_improvement: Option<Decimal>,
}

/// Parameters for running entry strategy simulations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationParams {
    /// Opening price of the window.
    pub open_price: Decimal,
    /// Closing price of the window.
    pub close_price: Decimal,
    /// Duration of the betting window.
    pub duration: Duration,
    /// Number of price paths to simulate.
    pub n_paths: usize,
    /// Estimated probability of Yes outcome.
    pub estimated_probability: Decimal,
    /// Bet direction.
    pub direction: BetDirection,
    /// Fee rate for trades.
    pub fee_rate: Decimal,
}

impl SimulationParams {
    /// Creates new simulation parameters.
    #[must_use]
    pub fn new(
        open_price: Decimal,
        close_price: Decimal,
        duration: Duration,
        n_paths: usize,
        estimated_probability: Decimal,
        direction: BetDirection,
        fee_rate: Decimal,
    ) -> Self {
        Self {
            open_price,
            close_price,
            duration,
            n_paths,
            estimated_probability,
            direction,
            fee_rate,
        }
    }

    /// Creates parameters for a 15-minute window with common defaults.
    #[must_use]
    pub fn default_15min(
        open_price: Decimal,
        close_price: Decimal,
        estimated_probability: Decimal,
        direction: BetDirection,
    ) -> Self {
        Self {
            open_price,
            close_price,
            duration: Duration::minutes(15),
            n_paths: 100,
            estimated_probability,
            direction,
            fee_rate: dec!(0.02),
        }
    }
}

/// Simulates entry strategies over price paths for comparison.
pub struct EntryStrategySimulator {
    /// Price path generator.
    generator: PricePathGenerator,
}

impl EntryStrategySimulator {
    /// Creates a new simulator with the given generator.
    #[must_use]
    pub fn new(generator: PricePathGenerator) -> Self {
        Self { generator }
    }

    /// Creates a simulator with default settings.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(PricePathGenerator::with_defaults())
    }

    /// Simulates an entry strategy over a single price path.
    pub fn simulate_single(
        &self,
        strategy: &dyn EntryStrategy,
        path: &PricePath,
        estimated_probability: Decimal,
        direction: BetDirection,
        fee_rate: Decimal,
    ) -> EntrySimulationResult {
        let mut result = EntrySimulationResult {
            strategy_name: strategy.name().to_string(),
            decision: EntryDecision::no_entry("No entry point found"),
            entry_price: None,
            edge_at_entry: None,
            open_price: path.open_price,
            close_price: path.close_price,
        };

        // Walk through price path and evaluate strategy at each point
        for point in &path.points {
            let ctx = EntryContext::new(
                point.price,
                estimated_probability,
                point.offset,
                path.duration,
                direction,
                fee_rate,
            );

            let decision = strategy.evaluate(&ctx);
            if decision.is_entry() {
                result.decision = decision;
                result.entry_price = Some(point.price);
                result.edge_at_entry = Some(ctx.calculate_edge());
                break;
            }
        }

        result
    }

    /// Simulates an entry strategy over multiple price paths.
    pub fn simulate_multiple(
        &self,
        strategy: &dyn EntryStrategy,
        params: &SimulationParams,
    ) -> Vec<EntrySimulationResult> {
        let paths = self.generator.generate_paths(
            params.open_price,
            params.close_price,
            params.duration,
            params.n_paths,
        );

        paths
            .iter()
            .map(|path| {
                self.simulate_single(
                    strategy,
                    path,
                    params.estimated_probability,
                    params.direction,
                    params.fee_rate,
                )
            })
            .collect()
    }

    /// Computes aggregate statistics from simulation results.
    #[must_use]
    pub fn compute_stats(
        results: &[EntrySimulationResult],
        direction: BetDirection,
    ) -> EntryStrategyStats {
        if results.is_empty() {
            return EntryStrategyStats {
                strategy_name: String::new(),
                n_simulations: 0,
                n_entries: 0,
                entry_rate: 0.0,
                avg_entry_price: None,
                avg_edge_at_entry: None,
                avg_price_improvement: None,
            };
        }

        let n_simulations = results.len();
        let entries: Vec<_> = results.iter().filter(|r| r.did_enter()).collect();
        let n_entries = entries.len();
        let entry_rate = n_entries as f64 / n_simulations as f64;

        let avg_entry_price = if n_entries > 0 {
            let sum: Decimal = entries.iter().filter_map(|r| r.entry_price).sum();
            Some(sum / Decimal::from(n_entries))
        } else {
            None
        };

        let avg_edge_at_entry = if n_entries > 0 {
            let sum: Decimal = entries.iter().filter_map(|r| r.edge_at_entry).sum();
            Some(sum / Decimal::from(n_entries))
        } else {
            None
        };

        let avg_price_improvement = if n_entries > 0 {
            let sum: Decimal = entries
                .iter()
                .filter_map(|r| r.price_improvement(direction))
                .sum();
            Some(sum / Decimal::from(n_entries))
        } else {
            None
        };

        EntryStrategyStats {
            strategy_name: results
                .first()
                .map(|r| r.strategy_name.clone())
                .unwrap_or_default(),
            n_simulations,
            n_entries,
            entry_rate,
            avg_entry_price,
            avg_edge_at_entry,
            avg_price_improvement,
        }
    }

    /// Compares multiple strategies by simulating each over the same paths.
    pub fn compare_strategies(
        &self,
        strategies: &[&dyn EntryStrategy],
        params: &SimulationParams,
    ) -> Vec<EntryStrategyStats> {
        let paths = self.generator.generate_paths(
            params.open_price,
            params.close_price,
            params.duration,
            params.n_paths,
        );

        strategies
            .iter()
            .map(|strategy| {
                let results: Vec<_> = paths
                    .iter()
                    .map(|path| {
                        self.simulate_single(
                            *strategy,
                            path,
                            params.estimated_probability,
                            params.direction,
                            params.fee_rate,
                        )
                    })
                    .collect();
                Self::compute_stats(&results, params.direction)
            })
            .collect()
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    // ============================================================
    // EntryContext Tests
    // ============================================================

    #[test]
    fn entry_context_calculate_edge_returns_correct_value_for_yes_bet() {
        // Yes bet: edge = P(yes) - price - fee_cost
        // P(yes) = 0.60, price = 0.50, fee = 0.02
        // fee_cost = 0.02 * 0.50 = 0.01
        // edge = 0.60 - 0.50 - 0.01 = 0.09
        let ctx = EntryContext::new(
            dec!(0.50),            // current_price
            dec!(0.60),            // estimated_probability
            Duration::minutes(5),  // current_offset
            Duration::minutes(15), // window_duration
            BetDirection::Yes,
            dec!(0.02), // fee_rate (2%)
        );

        let edge = ctx.calculate_edge();
        assert_eq!(edge, dec!(0.09));
    }

    #[test]
    fn entry_context_calculate_edge_returns_correct_value_for_no_bet() {
        // No bet: P(no) = 1 - P(yes) = 0.40, price for No = 1 - 0.50 = 0.50
        // edge = 0.40 - 0.50 - 0.02 * 0.50 = 0.40 - 0.50 - 0.01 = -0.11
        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.60), // P(yes), so P(no) = 0.40
            Duration::minutes(5),
            Duration::minutes(15),
            BetDirection::No,
            dec!(0.02),
        );

        let edge = ctx.calculate_edge();
        assert_eq!(edge, dec!(-0.11));
    }

    #[test]
    fn entry_context_calculate_edge_zero_fees() {
        let ctx = EntryContext::new(
            dec!(0.45),
            dec!(0.55),
            Duration::minutes(0),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        // edge = 0.55 - 0.45 - 0 = 0.10
        let edge = ctx.calculate_edge();
        assert_eq!(edge, dec!(0.10));
    }

    #[test]
    fn entry_context_remaining_time_calculated_correctly() {
        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.55),
            Duration::minutes(5),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        assert_eq!(ctx.remaining_time(), Duration::minutes(10));
    }

    #[test]
    fn entry_context_past_midpoint_returns_false_before_midpoint() {
        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.55),
            Duration::minutes(5),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        assert!(!ctx.past_midpoint());
    }

    #[test]
    fn entry_context_past_midpoint_returns_true_after_midpoint() {
        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.55),
            Duration::minutes(10),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        assert!(ctx.past_midpoint());
    }

    // ============================================================
    // FixedTimeEntry Tests
    // ============================================================

    #[test]
    fn fixed_time_entry_enters_at_specified_offset() {
        let strategy = FixedTimeEntry::new(Duration::minutes(5), Duration::minutes(14));

        // At offset 5 minutes, should enter
        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.55),
            Duration::minutes(5),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        let decision = strategy.evaluate(&ctx);
        assert!(decision.is_entry());

        if let EntryDecision::Enter { offset, direction } = decision {
            assert_eq!(offset, Duration::minutes(5));
            assert_eq!(direction, BetDirection::Yes);
        } else {
            panic!("Expected Entry decision");
        }
    }

    #[test]
    fn fixed_time_entry_waits_before_offset() {
        let strategy = FixedTimeEntry::new(Duration::minutes(5), Duration::minutes(14));

        // At offset 3 minutes, should wait
        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.55),
            Duration::minutes(3),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        let decision = strategy.evaluate(&ctx);
        assert!(!decision.is_entry());
    }

    #[test]
    fn fixed_time_entry_respects_cutoff() {
        let strategy = FixedTimeEntry::new(Duration::minutes(5), Duration::minutes(10));

        // At offset 12 minutes (past cutoff of 10), should not enter
        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.55),
            Duration::minutes(12),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        let decision = strategy.evaluate(&ctx);
        assert!(!decision.is_entry());
        if let EntryDecision::NoEntry { reason } = decision {
            assert!(reason.contains("cutoff"));
        }
    }

    #[test]
    fn fixed_time_entry_at_open_enters_immediately() {
        let strategy = FixedTimeEntry::at_open();

        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.55),
            Duration::zero(),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        let decision = strategy.evaluate(&ctx);
        assert!(decision.is_entry());
    }

    // ============================================================
    // EdgeThresholdEntry Tests
    // ============================================================

    #[test]
    fn edge_threshold_entry_enters_when_edge_exceeds_threshold() {
        let strategy = EdgeThresholdEntry::new(dec!(0.05), Duration::minutes(14));

        // Edge = 0.55 - 0.45 = 0.10 > 0.05 threshold
        let ctx = EntryContext::new(
            dec!(0.45),
            dec!(0.55),
            Duration::minutes(2),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        let decision = strategy.evaluate(&ctx);
        assert!(decision.is_entry());
    }

    #[test]
    fn edge_threshold_entry_does_not_enter_below_threshold() {
        let strategy = EdgeThresholdEntry::new(dec!(0.10), Duration::minutes(14));

        // Edge = 0.55 - 0.50 = 0.05 < 0.10 threshold
        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.55),
            Duration::minutes(2),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        let decision = strategy.evaluate(&ctx);
        assert!(!decision.is_entry());
        if let EntryDecision::NoEntry { reason } = decision {
            assert!(reason.contains("below threshold"));
        }
    }

    #[test]
    fn edge_threshold_entry_respects_cutoff() {
        let strategy = EdgeThresholdEntry::new(dec!(0.01), Duration::minutes(10));

        // Good edge but past cutoff
        let ctx = EntryContext::new(
            dec!(0.40),
            dec!(0.60),
            Duration::minutes(12),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        let decision = strategy.evaluate(&ctx);
        assert!(!decision.is_entry());
    }

    #[test]
    fn edge_threshold_entry_accounts_for_fees() {
        let strategy = EdgeThresholdEntry::new(dec!(0.05), Duration::minutes(14));

        // Without fees: edge = 0.55 - 0.48 = 0.07 > 0.05 (would enter)
        // With 5% fees: fee_cost = 0.05 * 0.48 = 0.024, edge = 0.07 - 0.024 = 0.046 < 0.05
        let ctx = EntryContext::new(
            dec!(0.48),
            dec!(0.55),
            Duration::minutes(2),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.05), // 5% fee
        );

        let decision = strategy.evaluate(&ctx);
        assert!(!decision.is_entry());
    }

    // ============================================================
    // PricePath Tests
    // ============================================================

    #[test]
    fn price_path_price_at_offset_returns_correct_price() {
        let points = vec![
            PricePoint::new(Duration::zero(), dec!(0.50)),
            PricePoint::new(Duration::minutes(5), dec!(0.52)),
            PricePoint::new(Duration::minutes(10), dec!(0.48)),
            PricePoint::new(Duration::minutes(15), dec!(0.51)),
        ];
        let path = PricePath::new(points);

        // Exact match
        assert_eq!(path.price_at_offset(Duration::minutes(5)), Some(dec!(0.52)));

        // At open
        assert_eq!(path.price_at_offset(Duration::zero()), Some(dec!(0.50)));

        // At close
        assert_eq!(
            path.price_at_offset(Duration::minutes(15)),
            Some(dec!(0.51))
        );
    }

    #[test]
    fn price_path_price_at_offset_interpolates_correctly() {
        let points = vec![
            PricePoint::new(Duration::zero(), dec!(0.50)),
            PricePoint::new(Duration::minutes(10), dec!(0.60)),
        ];
        let path = PricePath::new(points);

        // Midpoint should be 0.55
        let mid_price = path.price_at_offset(Duration::minutes(5)).unwrap();
        assert!(
            (mid_price - dec!(0.55)).abs() < dec!(0.001),
            "Expected ~0.55, got {}",
            mid_price
        );
    }

    #[test]
    fn price_path_constant_creates_flat_path() {
        let path = PricePath::constant(dec!(0.50), Duration::minutes(15), 5);

        assert_eq!(path.open_price, dec!(0.50));
        assert_eq!(path.close_price, dec!(0.50));
        assert_eq!(path.points.len(), 5);

        for point in &path.points {
            assert_eq!(point.price, dec!(0.50));
        }
    }

    #[test]
    fn price_path_min_max_calculated_correctly() {
        let points = vec![
            PricePoint::new(Duration::zero(), dec!(0.50)),
            PricePoint::new(Duration::minutes(5), dec!(0.45)),
            PricePoint::new(Duration::minutes(10), dec!(0.60)),
            PricePoint::new(Duration::minutes(15), dec!(0.52)),
        ];
        let path = PricePath::new(points);

        assert_eq!(path.min_price(), dec!(0.45));
        assert_eq!(path.max_price(), dec!(0.60));
        assert_eq!(path.price_range(), dec!(0.15));
    }

    // ============================================================
    // PricePathGenerator Tests
    // ============================================================

    #[test]
    fn price_path_generator_generates_path_with_correct_endpoints() {
        let config = PricePathConfig::new(50, 0.02).with_seed(42);
        let generator = PricePathGenerator::new(config);

        let path =
            generator.generate_brownian_bridge(dec!(0.50), dec!(0.55), Duration::minutes(15));

        assert_eq!(path.open_price, dec!(0.50));
        assert_eq!(path.close_price, dec!(0.55));
        assert_eq!(path.points.len(), 50);
    }

    #[test]
    fn brownian_bridge_anchors_at_start_and_end() {
        let config = PricePathConfig::new(100, 0.05).with_seed(123);
        let generator = PricePathGenerator::new(config);

        let path =
            generator.generate_brownian_bridge(dec!(0.45), dec!(0.60), Duration::minutes(15));

        // First point must be exactly open price
        assert_eq!(path.points.first().unwrap().price, dec!(0.45));

        // Last point must be exactly close price
        assert_eq!(path.points.last().unwrap().price, dec!(0.60));
    }

    #[test]
    fn brownian_bridge_is_reproducible_with_same_seed() {
        let config1 = PricePathConfig::new(50, 0.03).with_seed(999);
        let generator1 = PricePathGenerator::new(config1);
        let path1 =
            generator1.generate_brownian_bridge(dec!(0.50), dec!(0.55), Duration::minutes(15));

        let config2 = PricePathConfig::new(50, 0.03).with_seed(999);
        let generator2 = PricePathGenerator::new(config2);
        let path2 =
            generator2.generate_brownian_bridge(dec!(0.50), dec!(0.55), Duration::minutes(15));

        for (p1, p2) in path1.points.iter().zip(path2.points.iter()) {
            assert_eq!(p1.price, p2.price);
        }
    }

    #[test]
    fn brownian_bridge_different_seeds_produce_different_paths() {
        let config1 = PricePathConfig::new(50, 0.03).with_seed(111);
        let generator1 = PricePathGenerator::new(config1);
        let path1 =
            generator1.generate_brownian_bridge(dec!(0.50), dec!(0.55), Duration::minutes(15));

        let config2 = PricePathConfig::new(50, 0.03).with_seed(222);
        let generator2 = PricePathGenerator::new(config2);
        let path2 =
            generator2.generate_brownian_bridge(dec!(0.50), dec!(0.55), Duration::minutes(15));

        // Middle points should differ (endpoints are fixed)
        let mid_idx = 25;
        assert_ne!(path1.points[mid_idx].price, path2.points[mid_idx].price);
    }

    #[test]
    fn brownian_bridge_prices_stay_in_valid_range() {
        let config = PricePathConfig::new(100, 0.10).with_seed(42); // High volatility
        let generator = PricePathGenerator::new(config);

        let path =
            generator.generate_brownian_bridge(dec!(0.50), dec!(0.50), Duration::minutes(15));

        for point in &path.points {
            assert!(point.price >= dec!(0.01), "Price too low: {}", point.price);
            assert!(point.price <= dec!(0.99), "Price too high: {}", point.price);
        }
    }

    #[test]
    fn generate_multiple_paths_returns_correct_count() {
        let config = PricePathConfig::new(20, 0.02).with_seed(42);
        let generator = PricePathGenerator::new(config);

        let paths = generator.generate_paths(dec!(0.50), dec!(0.55), Duration::minutes(15), 10);

        assert_eq!(paths.len(), 10);

        // Each path should have correct endpoints
        for path in &paths {
            assert_eq!(path.open_price, dec!(0.50));
            assert_eq!(path.close_price, dec!(0.55));
        }
    }

    // ============================================================
    // EntryStrategySimulator Tests
    // ============================================================

    #[test]
    fn simulator_simulate_single_finds_entry() {
        let config = PricePathConfig::new(15, 0.0).with_seed(42);
        let generator = PricePathGenerator::new(config);
        let simulator = EntryStrategySimulator::new(generator);

        // Constant price path
        let path = PricePath::constant(dec!(0.50), Duration::minutes(15), 15);

        let strategy = FixedTimeEntry::new(Duration::minutes(5), Duration::minutes(14));
        let result =
            simulator.simulate_single(&strategy, &path, dec!(0.60), BetDirection::Yes, dec!(0.0));

        assert!(result.did_enter());
        assert_eq!(result.entry_price, Some(dec!(0.50)));
        assert!(result.edge_at_entry.is_some());
    }

    #[test]
    fn simulator_compute_stats_calculates_correctly() {
        let results = vec![
            EntrySimulationResult {
                strategy_name: "test".to_string(),
                decision: EntryDecision::enter(Duration::minutes(5), BetDirection::Yes),
                entry_price: Some(dec!(0.48)),
                edge_at_entry: Some(dec!(0.10)),
                open_price: dec!(0.50),
                close_price: dec!(0.52),
            },
            EntrySimulationResult {
                strategy_name: "test".to_string(),
                decision: EntryDecision::enter(Duration::minutes(3), BetDirection::Yes),
                entry_price: Some(dec!(0.46)),
                edge_at_entry: Some(dec!(0.12)),
                open_price: dec!(0.50),
                close_price: dec!(0.52),
            },
            EntrySimulationResult {
                strategy_name: "test".to_string(),
                decision: EntryDecision::no_entry("Past cutoff"),
                entry_price: None,
                edge_at_entry: None,
                open_price: dec!(0.50),
                close_price: dec!(0.52),
            },
        ];

        let stats = EntryStrategySimulator::compute_stats(&results, BetDirection::Yes);

        assert_eq!(stats.n_simulations, 3);
        assert_eq!(stats.n_entries, 2);
        assert!((stats.entry_rate - 0.6667).abs() < 0.01);
        assert_eq!(stats.avg_entry_price, Some(dec!(0.47)));
        assert_eq!(stats.avg_edge_at_entry, Some(dec!(0.11)));
    }

    #[test]
    fn simulator_compare_strategies_returns_stats_for_each() {
        let config = PricePathConfig::new(15, 0.01).with_seed(42);
        let generator = PricePathGenerator::new(config);
        let simulator = EntryStrategySimulator::new(generator);

        let strategy1 = FixedTimeEntry::at_open();
        let strategy2 = EdgeThresholdEntry::with_five_percent_edge();

        let strategies: Vec<&dyn EntryStrategy> = vec![&strategy1, &strategy2];
        let params = SimulationParams::new(
            dec!(0.50),
            dec!(0.52),
            Duration::minutes(15),
            10,
            dec!(0.55),
            BetDirection::Yes,
            dec!(0.0),
        );
        let stats = simulator.compare_strategies(&strategies, &params);

        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].strategy_name, "FixedTimeEntry");
        assert_eq!(stats[1].strategy_name, "EdgeThresholdEntry");
    }

    // ============================================================
    // EntryDecision Tests
    // ============================================================

    #[test]
    fn entry_decision_enter_is_entry() {
        let decision = EntryDecision::enter(Duration::minutes(5), BetDirection::Yes);
        assert!(decision.is_entry());
    }

    #[test]
    fn entry_decision_no_entry_is_not_entry() {
        let decision = EntryDecision::no_entry("test reason");
        assert!(!decision.is_entry());
    }

    // ============================================================
    // Price Improvement Tests
    // ============================================================

    #[test]
    fn price_improvement_positive_for_lower_yes_price() {
        let result = EntrySimulationResult {
            strategy_name: "test".to_string(),
            decision: EntryDecision::enter(Duration::minutes(5), BetDirection::Yes),
            entry_price: Some(dec!(0.45)),
            edge_at_entry: Some(dec!(0.10)),
            open_price: dec!(0.50),
            close_price: dec!(0.52),
        };

        // For Yes bet, lower price = better, so improvement = 0.50 - 0.45 = 0.05
        let improvement = result.price_improvement(BetDirection::Yes);
        assert_eq!(improvement, Some(dec!(0.05)));
    }

    #[test]
    fn price_improvement_positive_for_higher_no_price() {
        let result = EntrySimulationResult {
            strategy_name: "test".to_string(),
            decision: EntryDecision::enter(Duration::minutes(5), BetDirection::No),
            entry_price: Some(dec!(0.55)),
            edge_at_entry: Some(dec!(0.10)),
            open_price: dec!(0.50),
            close_price: dec!(0.52),
        };

        // For No bet, higher Yes price = better (lower No price), improvement = 0.55 - 0.50 = 0.05
        let improvement = result.price_improvement(BetDirection::No);
        assert_eq!(improvement, Some(dec!(0.05)));
    }

    #[test]
    fn price_improvement_none_when_no_entry() {
        let result = EntrySimulationResult {
            strategy_name: "test".to_string(),
            decision: EntryDecision::no_entry("test"),
            entry_price: None,
            edge_at_entry: None,
            open_price: dec!(0.50),
            close_price: dec!(0.52),
        };

        assert!(result.price_improvement(BetDirection::Yes).is_none());
    }

    // ============================================================
    // Edge Cases
    // ============================================================

    #[test]
    fn price_path_empty_returns_none_for_price_at_offset() {
        let path = PricePath {
            points: vec![],
            open_price: Decimal::ZERO,
            close_price: Decimal::ZERO,
            duration: Duration::zero(),
        };

        assert!(path.price_at_offset(Duration::minutes(5)).is_none());
    }

    #[test]
    fn compute_stats_empty_results_returns_zeros() {
        let stats = EntryStrategySimulator::compute_stats(&[], BetDirection::Yes);

        assert_eq!(stats.n_simulations, 0);
        assert_eq!(stats.n_entries, 0);
        assert!((stats.entry_rate - 0.0).abs() < f64::EPSILON);
        assert!(stats.avg_entry_price.is_none());
    }

    #[test]
    fn fixed_time_entry_exactly_at_cutoff_does_not_enter() {
        let strategy = FixedTimeEntry::new(Duration::minutes(5), Duration::minutes(10));

        // Exactly at cutoff
        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.55),
            Duration::minutes(10), // Equal to cutoff
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        let decision = strategy.evaluate(&ctx);
        assert!(!decision.is_entry());
    }

    #[test]
    fn edge_threshold_exactly_at_threshold_enters() {
        let strategy = EdgeThresholdEntry::new(dec!(0.05), Duration::minutes(14));

        // Edge = 0.55 - 0.50 = 0.05 exactly at threshold
        let ctx = EntryContext::new(
            dec!(0.50),
            dec!(0.55),
            Duration::minutes(2),
            Duration::minutes(15),
            BetDirection::Yes,
            dec!(0.0),
        );

        let decision = strategy.evaluate(&ctx);
        assert!(decision.is_entry());
    }

    // ============================================================
    // Integration Test: Full Simulation Flow
    // ============================================================

    #[test]
    fn integration_full_simulation_flow() {
        // Setup generator with known seed for reproducibility
        let config = PricePathConfig::new(30, 0.02).with_seed(12345);
        let generator = PricePathGenerator::new(config);
        let simulator = EntryStrategySimulator::new(generator);

        // Create two strategies to compare
        let immediate = FixedTimeEntry::at_open();
        let edge_based = EdgeThresholdEntry::new(dec!(0.08), Duration::minutes(13));

        let strategies: Vec<&dyn EntryStrategy> = vec![&immediate, &edge_based];

        // Simulate over multiple paths
        let params = SimulationParams::new(
            dec!(0.50),
            dec!(0.55),
            Duration::minutes(15),
            50,
            dec!(0.60), // P(yes) = 60%
            BetDirection::Yes,
            dec!(0.02), // 2% fees
        );
        let stats = simulator.compare_strategies(&strategies, &params);

        // Immediate entry should always enter
        assert_eq!(stats[0].n_entries, 50);
        assert!((stats[0].entry_rate - 1.0).abs() < f64::EPSILON);

        // Edge-based may not always enter
        assert!(stats[1].n_entries <= 50);

        // If edge-based entered, it should have better edge on average
        // (though this is probabilistic so we just verify structure)
        assert!(
            stats[1].avg_edge_at_entry.is_none()
                || stats[1].avg_edge_at_entry.unwrap() >= dec!(0.08)
        );
    }
}
