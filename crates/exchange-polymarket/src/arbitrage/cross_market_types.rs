//! Cross-market correlation arbitrage types.
//!
//! Types for detecting opportunities across multiple coin 15-minute markets.
//! The strategy exploits correlation between crypto assets (BTC, ETH, SOL, XRP).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use crate::models::Coin;

/// A cross-market pair (e.g., BTC/ETH).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoinPair {
    pub coin1: Coin,
    pub coin2: Coin,
}

impl CoinPair {
    /// Creates a new coin pair.
    #[must_use]
    pub fn new(coin1: Coin, coin2: Coin) -> Self {
        Self { coin1, coin2 }
    }

    /// Generate all unique pairs from a list of coins.
    /// For 4 coins, generates 6 pairs: (0,1), (0,2), (0,3), (1,2), (1,3), (2,3)
    #[must_use]
    pub fn all_pairs(coins: &[Coin]) -> Vec<Self> {
        let mut pairs = Vec::new();
        for i in 0..coins.len() {
            for j in (i + 1)..coins.len() {
                pairs.push(Self::new(coins[i], coins[j]));
            }
        }
        pairs
    }

    /// Returns a display name for the pair (e.g., "BTC/ETH").
    #[must_use]
    pub fn display_name(&self) -> String {
        format!(
            "{}/{}",
            self.coin1.slug_prefix().to_uppercase(),
            self.coin2.slug_prefix().to_uppercase()
        )
    }
}

impl fmt::Display for CoinPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// The combination type for a cross-market bet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CrossMarketCombination {
    /// Coin1 UP + Coin2 DOWN (opposite direction bet)
    Coin1UpCoin2Down,
    /// Coin1 DOWN + Coin2 UP (opposite direction bet)
    Coin1DownCoin2Up,
    /// Coin1 UP + Coin2 UP (same direction bet)
    BothUp,
    /// Coin1 DOWN + Coin2 DOWN (same direction bet)
    BothDown,
}

impl CrossMarketCombination {
    /// Returns all combinations.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Coin1UpCoin2Down,
            Self::Coin1DownCoin2Up,
            Self::BothUp,
            Self::BothDown,
        ]
    }

    /// Returns whether this is an opposite-direction bet.
    #[must_use]
    pub fn is_opposite_direction(&self) -> bool {
        matches!(self, Self::Coin1UpCoin2Down | Self::Coin1DownCoin2Up)
    }

    /// Returns the directions for each leg as (leg1_up, leg2_up).
    #[must_use]
    pub fn directions(&self) -> (bool, bool) {
        match self {
            Self::Coin1UpCoin2Down => (true, false),
            Self::Coin1DownCoin2Up => (false, true),
            Self::BothUp => (true, true),
            Self::BothDown => (false, false),
        }
    }
}

impl fmt::Display for CrossMarketCombination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Coin1UpCoin2Down => write!(f, "Coin1UpCoin2Down"),
            Self::Coin1DownCoin2Up => write!(f, "Coin1DownCoin2Up"),
            Self::BothUp => write!(f, "BothUp"),
            Self::BothDown => write!(f, "BothDown"),
        }
    }
}

/// Order book depth data for a single token.
#[derive(Debug, Clone, Default)]
pub struct TokenDepth {
    /// Total bid depth (shares available to sell into).
    pub bid_depth: Decimal,
    /// Total ask depth (shares available to buy).
    pub ask_depth: Decimal,
    /// Spread in basis points.
    pub spread_bps: Decimal,
}

/// Snapshot of a single coin's market prices.
#[derive(Debug, Clone)]
pub struct CoinMarketSnapshot {
    /// The coin type.
    pub coin: Coin,
    /// UP outcome price (probability of going up).
    pub up_price: Decimal,
    /// DOWN outcome price (probability of going down).
    pub down_price: Decimal,
    /// Token ID for UP outcome.
    pub up_token_id: String,
    /// Token ID for DOWN outcome.
    pub down_token_id: String,
    /// Snapshot timestamp in milliseconds.
    pub timestamp_ms: i64,
    /// Order book depth for UP token (optional, populated if WebSocket connected).
    pub up_depth: Option<TokenDepth>,
    /// Order book depth for DOWN token (optional, populated if WebSocket connected).
    pub down_depth: Option<TokenDepth>,
}

impl CoinMarketSnapshot {
    /// Returns the price for the specified direction.
    #[must_use]
    pub fn price_for_direction(&self, is_up: bool) -> Decimal {
        if is_up {
            self.up_price
        } else {
            self.down_price
        }
    }

    /// Returns the token ID for the specified direction.
    #[must_use]
    pub fn token_for_direction(&self, is_up: bool) -> &str {
        if is_up {
            &self.up_token_id
        } else {
            &self.down_token_id
        }
    }

    /// Returns the order book depth for the specified direction.
    #[must_use]
    pub fn depth_for_direction(&self, is_up: bool) -> Option<&TokenDepth> {
        if is_up {
            self.up_depth.as_ref()
        } else {
            self.down_depth.as_ref()
        }
    }
}

/// A detected cross-market opportunity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossMarketOpportunity {
    /// First coin in the pair.
    pub coin1: String,
    /// Second coin in the pair.
    pub coin2: String,

    /// The combination type.
    pub combination: CrossMarketCombination,

    /// Leg 1 direction ("UP" or "DOWN").
    pub leg1_direction: String,
    /// Leg 1 price.
    pub leg1_price: Decimal,
    /// Leg 1 token ID.
    pub leg1_token_id: String,

    /// Leg 2 direction ("UP" or "DOWN").
    pub leg2_direction: String,
    /// Leg 2 price.
    pub leg2_price: Decimal,
    /// Leg 2 token ID.
    pub leg2_token_id: String,

    /// Total cost to enter both positions.
    pub total_cost: Decimal,

    /// Spread below $1.00 (guaranteed minimum profit if both win).
    pub spread: Decimal,

    /// Expected value based on correlation assumption.
    pub expected_value: Decimal,

    /// Assumed correlation between the coins.
    pub assumed_correlation: f64,

    /// P(at least one wins) based on correlation.
    pub win_probability: f64,

    /// Detection timestamp.
    pub detected_at: DateTime<Utc>,

    // === Order Book Depth Fields ===
    /// Leg 1 bid depth (total shares available).
    pub leg1_bid_depth: Option<Decimal>,
    /// Leg 1 ask depth (total shares available).
    pub leg1_ask_depth: Option<Decimal>,
    /// Leg 1 spread in basis points.
    pub leg1_spread_bps: Option<Decimal>,

    /// Leg 2 bid depth (total shares available).
    pub leg2_bid_depth: Option<Decimal>,
    /// Leg 2 ask depth (total shares available).
    pub leg2_ask_depth: Option<Decimal>,
    /// Leg 2 spread in basis points.
    pub leg2_spread_bps: Option<Decimal>,
}

impl CrossMarketOpportunity {
    /// Returns the ROI if we win (payout / cost - 1).
    #[must_use]
    pub fn roi(&self) -> Decimal {
        if self.total_cost == Decimal::ZERO {
            return Decimal::ZERO;
        }
        (Decimal::ONE - self.total_cost) / self.total_cost
    }

    /// Returns a short display string for the opportunity.
    #[must_use]
    pub fn display_short(&self) -> String {
        format!(
            "{} {} + {} {} = ${:.2} (spread: ${:.2}, EV: ${:.3})",
            self.coin1,
            self.leg1_direction,
            self.coin2,
            self.leg2_direction,
            self.total_cost,
            self.spread,
            self.expected_value
        )
    }

    /// Returns true if order book depth data is available.
    #[must_use]
    pub fn has_depth_data(&self) -> bool {
        self.leg1_bid_depth.is_some() && self.leg2_bid_depth.is_some()
    }

    /// Returns the minimum depth across all legs.
    #[must_use]
    pub fn min_depth(&self) -> Option<Decimal> {
        match (
            self.leg1_bid_depth,
            self.leg1_ask_depth,
            self.leg2_bid_depth,
            self.leg2_ask_depth,
        ) {
            (Some(l1b), Some(l1a), Some(l2b), Some(l2a)) => Some(l1b.min(l1a).min(l2b).min(l2a)),
            _ => None,
        }
    }

    /// Sets depth data from token depth info.
    pub fn with_depth(
        mut self,
        leg1_depth: Option<&TokenDepth>,
        leg2_depth: Option<&TokenDepth>,
    ) -> Self {
        if let Some(d) = leg1_depth {
            self.leg1_bid_depth = Some(d.bid_depth);
            self.leg1_ask_depth = Some(d.ask_depth);
            self.leg1_spread_bps = Some(d.spread_bps);
        }
        if let Some(d) = leg2_depth {
            self.leg2_bid_depth = Some(d.bid_depth);
            self.leg2_ask_depth = Some(d.ask_depth);
            self.leg2_spread_bps = Some(d.spread_bps);
        }
        self
    }

    /// Returns a display string with depth info.
    #[must_use]
    pub fn display_with_depth(&self) -> String {
        let depth_str = if self.has_depth_data() {
            format!(
                " [L1: {}b/{}a, L2: {}b/{}a]",
                self.leg1_bid_depth.unwrap_or(Decimal::ZERO),
                self.leg1_ask_depth.unwrap_or(Decimal::ZERO),
                self.leg2_bid_depth.unwrap_or(Decimal::ZERO),
                self.leg2_ask_depth.unwrap_or(Decimal::ZERO),
            )
        } else {
            String::new()
        };
        format!("{}{}", self.display_short(), depth_str)
    }
}

/// Configuration for the cross-market detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossMarketConfig {
    /// Maximum total cost to consider an opportunity (e.g., $0.95).
    pub max_total_cost: Decimal,

    /// Minimum spread ($1.00 - total_cost) to consider (e.g., $0.03).
    pub min_spread: Decimal,

    /// Assumed correlation between coins (0.0 to 1.0).
    pub assumed_correlation: f64,

    /// Minimum expected value to signal (e.g., $0.01).
    pub min_expected_value: Decimal,

    /// Signal cooldown in milliseconds.
    pub signal_cooldown_ms: i64,

    /// Coins to scan.
    pub coins: Vec<Coin>,

    /// Combinations to detect (None = all, Some = only specified).
    pub combinations: Option<Vec<CrossMarketCombination>>,

    /// Minimum order book depth (shares) on both legs to consider opportunity.
    /// Set to 0 to disable depth filtering (default).
    /// When > 0, opportunities with insufficient liquidity are filtered out.
    pub min_depth: Decimal,
}

impl Default for CrossMarketConfig {
    fn default() -> Self {
        Self {
            max_total_cost: dec!(0.95),
            min_spread: dec!(0.03),
            assumed_correlation: 0.85,
            min_expected_value: dec!(0.01),
            signal_cooldown_ms: 5_000,
            coins: vec![Coin::Btc, Coin::Eth, Coin::Sol, Coin::Xrp],
            combinations: None,       // All combinations
            min_depth: Decimal::ZERO, // No depth filtering by default
        }
    }
}

impl CrossMarketConfig {
    /// Creates an aggressive config (lower thresholds).
    #[must_use]
    pub fn aggressive() -> Self {
        Self {
            max_total_cost: dec!(0.98),
            min_spread: dec!(0.01),
            assumed_correlation: 0.80,
            min_expected_value: dec!(0.005),
            signal_cooldown_ms: 2_000,
            coins: vec![Coin::Btc, Coin::Eth, Coin::Sol, Coin::Xrp],
            combinations: None,
            min_depth: Decimal::ZERO,
        }
    }

    /// Creates a conservative config (higher thresholds).
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            max_total_cost: dec!(0.90),
            min_spread: dec!(0.05),
            assumed_correlation: 0.90,
            min_expected_value: dec!(0.02),
            signal_cooldown_ms: 10_000,
            coins: vec![Coin::Btc, Coin::Eth],
            combinations: None,
            min_depth: dec!(100), // Require at least 100 shares
        }
    }

    /// Only detect Coin1Up/Coin2Down combinations (high win rate strategy).
    #[must_use]
    pub fn only_up_down(mut self) -> Self {
        self.combinations = Some(vec![CrossMarketCombination::Coin1UpCoin2Down]);
        self
    }

    /// Only detect real arbitrage combinations (opposing directions).
    /// This includes both Coin1Up/Coin2Down and Coin1Down/Coin2Up.
    /// Excludes directional bets (BothUp, BothDown) which are not true arbitrage.
    #[must_use]
    pub fn arbitrage_only(mut self) -> Self {
        self.combinations = Some(vec![
            CrossMarketCombination::Coin1UpCoin2Down,
            CrossMarketCombination::Coin1DownCoin2Up,
        ]);
        self
    }
}

/// Statistics for the cross-market scanner.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrossMarketStats {
    /// Total checks performed.
    pub checks_performed: u64,
    /// Total opportunities found.
    pub opportunities_found: u64,
    /// Opportunities by combination type.
    pub by_combination: HashMap<String, u64>,
    /// Opportunities by coin pair.
    pub by_pair: HashMap<String, u64>,
    /// Best spread seen.
    pub best_spread_seen: Option<Decimal>,
    /// Lowest total cost seen.
    pub lowest_cost_seen: Option<Decimal>,
    /// Scanner start time.
    pub started_at: Option<DateTime<Utc>>,
}

impl CrossMarketStats {
    /// Records a new opportunity.
    pub fn record_opportunity(&mut self, opp: &CrossMarketOpportunity) {
        self.opportunities_found += 1;

        // Track by combination
        let combo_key = opp.combination.to_string();
        *self.by_combination.entry(combo_key).or_insert(0) += 1;

        // Track by pair
        let pair_key = format!("{}/{}", opp.coin1, opp.coin2);
        *self.by_pair.entry(pair_key).or_insert(0) += 1;

        // Track best spread
        if self.best_spread_seen.map_or(true, |best| opp.spread > best) {
            self.best_spread_seen = Some(opp.spread);
        }

        // Track lowest cost
        if self
            .lowest_cost_seen
            .map_or(true, |lowest| opp.total_cost < lowest)
        {
            self.lowest_cost_seen = Some(opp.total_cost);
        }
    }
}

// ============================================================================
// Tests (TDD - written first)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // CoinPair Tests
    // -------------------------------------------------------------------------

    #[test]
    fn coin_pair_new_creates_pair() {
        let pair = CoinPair::new(Coin::Btc, Coin::Eth);
        assert_eq!(pair.coin1, Coin::Btc);
        assert_eq!(pair.coin2, Coin::Eth);
    }

    #[test]
    fn coin_pair_all_pairs_generates_six_pairs_for_four_coins() {
        let coins = vec![Coin::Btc, Coin::Eth, Coin::Sol, Coin::Xrp];
        let pairs = CoinPair::all_pairs(&coins);

        // 4 choose 2 = 6 pairs
        assert_eq!(pairs.len(), 6);

        // Verify specific pairs exist
        assert!(pairs.contains(&CoinPair::new(Coin::Btc, Coin::Eth)));
        assert!(pairs.contains(&CoinPair::new(Coin::Btc, Coin::Sol)));
        assert!(pairs.contains(&CoinPair::new(Coin::Btc, Coin::Xrp)));
        assert!(pairs.contains(&CoinPair::new(Coin::Eth, Coin::Sol)));
        assert!(pairs.contains(&CoinPair::new(Coin::Eth, Coin::Xrp)));
        assert!(pairs.contains(&CoinPair::new(Coin::Sol, Coin::Xrp)));
    }

    #[test]
    fn coin_pair_all_pairs_empty_for_single_coin() {
        let coins = vec![Coin::Btc];
        let pairs = CoinPair::all_pairs(&coins);
        assert!(pairs.is_empty());
    }

    #[test]
    fn coin_pair_all_pairs_one_pair_for_two_coins() {
        let coins = vec![Coin::Btc, Coin::Eth];
        let pairs = CoinPair::all_pairs(&coins);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], CoinPair::new(Coin::Btc, Coin::Eth));
    }

    #[test]
    fn coin_pair_display_name() {
        let pair = CoinPair::new(Coin::Btc, Coin::Eth);
        assert_eq!(pair.display_name(), "BTC/ETH");
    }

    // -------------------------------------------------------------------------
    // CrossMarketCombination Tests
    // -------------------------------------------------------------------------

    #[test]
    fn combination_all_returns_four() {
        assert_eq!(CrossMarketCombination::all().len(), 4);
    }

    #[test]
    fn combination_opposite_direction_correct() {
        assert!(CrossMarketCombination::Coin1UpCoin2Down.is_opposite_direction());
        assert!(CrossMarketCombination::Coin1DownCoin2Up.is_opposite_direction());
        assert!(!CrossMarketCombination::BothUp.is_opposite_direction());
        assert!(!CrossMarketCombination::BothDown.is_opposite_direction());
    }

    #[test]
    fn combination_directions_correct() {
        assert_eq!(
            CrossMarketCombination::Coin1UpCoin2Down.directions(),
            (true, false)
        );
        assert_eq!(
            CrossMarketCombination::Coin1DownCoin2Up.directions(),
            (false, true)
        );
        assert_eq!(CrossMarketCombination::BothUp.directions(), (true, true));
        assert_eq!(
            CrossMarketCombination::BothDown.directions(),
            (false, false)
        );
    }

    // -------------------------------------------------------------------------
    // CoinMarketSnapshot Tests
    // -------------------------------------------------------------------------

    #[test]
    fn snapshot_price_for_direction() {
        let snapshot = CoinMarketSnapshot {
            coin: Coin::Btc,
            up_price: dec!(0.45),
            down_price: dec!(0.55),
            up_token_id: "up123".to_string(),
            down_token_id: "down456".to_string(),
            timestamp_ms: 1000,
            up_depth: None,
            down_depth: None,
        };

        assert_eq!(snapshot.price_for_direction(true), dec!(0.45));
        assert_eq!(snapshot.price_for_direction(false), dec!(0.55));
    }

    #[test]
    fn snapshot_token_for_direction() {
        let snapshot = CoinMarketSnapshot {
            coin: Coin::Eth,
            up_price: dec!(0.30),
            down_price: dec!(0.70),
            up_token_id: "eth_up".to_string(),
            down_token_id: "eth_down".to_string(),
            timestamp_ms: 2000,
            up_depth: None,
            down_depth: None,
        };

        assert_eq!(snapshot.token_for_direction(true), "eth_up");
        assert_eq!(snapshot.token_for_direction(false), "eth_down");
    }

    // -------------------------------------------------------------------------
    // CrossMarketConfig Tests
    // -------------------------------------------------------------------------

    #[test]
    fn config_default_values() {
        let config = CrossMarketConfig::default();
        assert_eq!(config.max_total_cost, dec!(0.95));
        assert_eq!(config.min_spread, dec!(0.03));
        assert!((config.assumed_correlation - 0.85).abs() < 0.001);
        assert_eq!(config.min_expected_value, dec!(0.01));
        assert_eq!(config.signal_cooldown_ms, 5_000);
        assert_eq!(config.coins.len(), 4);
    }

    #[test]
    fn config_aggressive_has_higher_cost_threshold() {
        let aggressive = CrossMarketConfig::aggressive();
        let default = CrossMarketConfig::default();
        assert!(aggressive.max_total_cost > default.max_total_cost);
        assert!(aggressive.min_spread < default.min_spread);
    }

    #[test]
    fn config_conservative_has_lower_cost_threshold() {
        let conservative = CrossMarketConfig::conservative();
        let default = CrossMarketConfig::default();
        assert!(conservative.max_total_cost < default.max_total_cost);
        assert!(conservative.min_spread > default.min_spread);
    }

    // -------------------------------------------------------------------------
    // CrossMarketOpportunity Tests
    // -------------------------------------------------------------------------

    // Helper to create test opportunity
    fn make_opp(
        coin1: &str,
        coin2: &str,
        combo: CrossMarketCombination,
        leg1_dir: &str,
        leg1_price: Decimal,
        leg2_dir: &str,
        leg2_price: Decimal,
    ) -> CrossMarketOpportunity {
        let total_cost = leg1_price + leg2_price;
        CrossMarketOpportunity {
            coin1: coin1.to_string(),
            coin2: coin2.to_string(),
            combination: combo,
            leg1_direction: leg1_dir.to_string(),
            leg1_price,
            leg1_token_id: "t1".to_string(),
            leg2_direction: leg2_dir.to_string(),
            leg2_price,
            leg2_token_id: "t2".to_string(),
            total_cost,
            spread: Decimal::ONE - total_cost,
            expected_value: dec!(0.03),
            assumed_correlation: 0.85,
            win_probability: 0.95,
            detected_at: Utc::now(),
            leg1_bid_depth: None,
            leg1_ask_depth: None,
            leg1_spread_bps: None,
            leg2_bid_depth: None,
            leg2_ask_depth: None,
            leg2_spread_bps: None,
        }
    }

    #[test]
    fn opportunity_roi_calculation() {
        let opp = make_opp(
            "BTC",
            "ETH",
            CrossMarketCombination::Coin1UpCoin2Down,
            "UP",
            dec!(0.05),
            "DOWN",
            dec!(0.91),
        );

        // ROI = (1.00 - 0.96) / 0.96 = 0.04 / 0.96 ≈ 0.0417
        let roi = opp.roi();
        assert!(roi > dec!(0.04));
        assert!(roi < dec!(0.05));
    }

    #[test]
    fn opportunity_roi_zero_cost_returns_zero() {
        let mut opp = make_opp(
            "BTC",
            "ETH",
            CrossMarketCombination::BothUp,
            "UP",
            Decimal::ZERO,
            "UP",
            Decimal::ZERO,
        );
        opp.total_cost = Decimal::ZERO;

        assert_eq!(opp.roi(), Decimal::ZERO);
    }

    // -------------------------------------------------------------------------
    // CrossMarketStats Tests
    // -------------------------------------------------------------------------

    #[test]
    fn stats_default_is_empty() {
        let stats = CrossMarketStats::default();
        assert_eq!(stats.checks_performed, 0);
        assert_eq!(stats.opportunities_found, 0);
        assert!(stats.by_combination.is_empty());
        assert!(stats.by_pair.is_empty());
        assert!(stats.best_spread_seen.is_none());
    }

    #[test]
    fn stats_record_opportunity_increments_count() {
        let mut stats = CrossMarketStats::default();
        let opp = make_opp(
            "BTC",
            "ETH",
            CrossMarketCombination::Coin1UpCoin2Down,
            "UP",
            dec!(0.05),
            "DOWN",
            dec!(0.91),
        );

        stats.record_opportunity(&opp);

        assert_eq!(stats.opportunities_found, 1);
        assert_eq!(stats.by_combination.get("C1_UP+C2_DOWN"), Some(&1));
        assert_eq!(stats.by_pair.get("BTC/ETH"), Some(&1));
        assert_eq!(stats.best_spread_seen, Some(dec!(0.04)));
        assert_eq!(stats.lowest_cost_seen, Some(dec!(0.96)));
    }

    #[test]
    fn stats_tracks_best_spread() {
        let mut stats = CrossMarketStats::default();

        let opp1 = make_opp(
            "BTC",
            "ETH",
            CrossMarketCombination::Coin1UpCoin2Down,
            "UP",
            dec!(0.10),
            "DOWN",
            dec!(0.85),
        );

        let opp2 = make_opp(
            "SOL",
            "XRP",
            CrossMarketCombination::BothDown,
            "DOWN",
            dec!(0.05),
            "DOWN",
            dec!(0.85),
        );

        stats.record_opportunity(&opp1);
        stats.record_opportunity(&opp2);

        assert_eq!(stats.best_spread_seen, Some(dec!(0.10))); // Should track best
        assert_eq!(stats.lowest_cost_seen, Some(dec!(0.90))); // Should track lowest
    }
}
