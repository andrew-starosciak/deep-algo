//! Integration tests for the arbitrage detection system.
//!
//! These tests verify end-to-end arbitrage detection scenarios including:
//! - Full arbitrage detection flow with realistic order books
//! - Profitable and non-profitable scenario validation
//! - Depth-limited pricing behavior
//! - Metrics accumulation and Wilson CI calculations
//! - Position tracking with YES/NO fills

use algo_trade_polymarket::arbitrage::metrics::{wilson_ci, ArbitrageMetrics, MIN_SAMPLE_SIZE};
use algo_trade_polymarket::arbitrage::{
    ArbitrageDetector, ArbitragePosition, L2OrderBook, PositionStatus,
};
use chrono::Utc;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use uuid::Uuid;

// =============================================================================
// Helper Functions
// =============================================================================

/// Creates a realistic YES order book for BTC 15-minute markets.
///
/// Simulates typical market maker activity with multiple price levels.
fn create_realistic_yes_orderbook(best_ask: Decimal, depth_per_level: Decimal) -> L2OrderBook {
    let mut book = L2OrderBook::new("yes-token-btc-15m".to_string());
    book.apply_snapshot(
        // Bids (below best ask)
        vec![
            (best_ask - dec!(0.02), depth_per_level),
            (best_ask - dec!(0.03), depth_per_level * dec!(1.5)),
            (best_ask - dec!(0.04), depth_per_level * dec!(2)),
        ],
        // Asks (starting at best ask)
        vec![
            (best_ask, depth_per_level),
            (best_ask + dec!(0.01), depth_per_level * dec!(1.5)),
            (best_ask + dec!(0.02), depth_per_level * dec!(2)),
        ],
    );
    book
}

/// Creates a realistic NO order book for BTC 15-minute markets.
fn create_realistic_no_orderbook(best_ask: Decimal, depth_per_level: Decimal) -> L2OrderBook {
    let mut book = L2OrderBook::new("no-token-btc-15m".to_string());
    book.apply_snapshot(
        vec![
            (best_ask - dec!(0.02), depth_per_level),
            (best_ask - dec!(0.03), depth_per_level * dec!(1.5)),
            (best_ask - dec!(0.04), depth_per_level * dec!(2)),
        ],
        vec![
            (best_ask, depth_per_level),
            (best_ask + dec!(0.01), depth_per_level * dec!(1.5)),
            (best_ask + dec!(0.02), depth_per_level * dec!(2)),
        ],
    );
    book
}

/// Creates an ArbitragePosition for testing.
fn create_test_position(
    yes_shares: Decimal,
    yes_cost: Decimal,
    no_shares: Decimal,
    no_cost: Decimal,
) -> ArbitragePosition {
    let yes_avg_price = if yes_shares > Decimal::ZERO {
        yes_cost / yes_shares
    } else {
        Decimal::ZERO
    };
    let no_avg_price = if no_shares > Decimal::ZERO {
        no_cost / no_shares
    } else {
        Decimal::ZERO
    };

    let min_shares = yes_shares.min(no_shares);
    let pair_cost = if min_shares > Decimal::ZERO {
        (yes_cost + no_cost) / min_shares
    } else {
        Decimal::MAX
    };

    ArbitragePosition {
        id: Uuid::new_v4(),
        market_id: "btc-15m-market-test".to_string(),
        yes_shares,
        yes_cost,
        yes_avg_price,
        no_shares,
        no_cost,
        no_avg_price,
        pair_cost,
        guaranteed_payout: min_shares,
        imbalance: yes_shares - no_shares,
        opened_at: Utc::now(),
        status: PositionStatus::Building,
    }
}

// =============================================================================
// Test 1: Full Arbitrage Detection Flow
// =============================================================================

/// Tests the complete arbitrage detection flow from order book creation
/// through opportunity detection and field validation.
#[test]
fn test_full_arbitrage_detection_flow() {
    // Arrange: Create order books with realistic BTC 15-min market data
    let yes_book = create_realistic_yes_orderbook(dec!(0.47), dec!(200));
    let no_book = create_realistic_no_orderbook(dec!(0.48), dec!(200));

    let detector = ArbitrageDetector::default();
    let order_size = dec!(100);

    // Act: Detect arbitrage opportunity
    let opportunity = detector
        .detect("btc-15m-up-12345", &yes_book, &no_book, order_size)
        .expect("Should detect arbitrage opportunity");

    // Assert: Verify all opportunity fields are correctly calculated
    assert_eq!(opportunity.market_id, "btc-15m-up-12345");
    assert_eq!(opportunity.yes_token_id, "yes-token-btc-15m");
    assert_eq!(opportunity.no_token_id, "no-token-btc-15m");

    // Pair cost = YES ask + NO ask = 0.47 + 0.48 = 0.95
    assert_eq!(opportunity.pair_cost, dec!(0.95));
    assert_eq!(opportunity.yes_worst_fill, dec!(0.47));
    assert_eq!(opportunity.no_worst_fill, dec!(0.48));

    // Gross profit = 1.00 - 0.95 = 0.05
    assert_eq!(opportunity.gross_profit_per_pair, dec!(0.05));

    // Expected fee = 0.01 * (2 - 0.95) = 0.01 * 1.05 = 0.0105
    assert_eq!(opportunity.expected_fee, dec!(0.0105));

    // Gas cost = 0.007 * 2 = 0.014
    assert_eq!(opportunity.gas_cost, dec!(0.014));

    // Net profit = 0.05 - 0.0105 - 0.014 = 0.0255
    assert_eq!(opportunity.net_profit_per_pair, dec!(0.0255));

    // Sizing fields
    assert_eq!(opportunity.recommended_size, dec!(100));
    assert_eq!(opportunity.total_investment, dec!(95)); // 100 * 0.95
    assert_eq!(opportunity.guaranteed_payout, dec!(100));

    // Depth fields
    assert_eq!(opportunity.yes_depth, dec!(100));
    assert_eq!(opportunity.no_depth, dec!(100));

    // ROI = net_profit / pair_cost * 100 = 0.0255 / 0.95 * 100
    let expected_roi = dec!(0.0255) / dec!(0.95) * dec!(100);
    assert_eq!(opportunity.roi, expected_roi);

    // Risk score should be low for single-level fill with good margin
    assert!(
        opportunity.risk_score < 0.3,
        "Risk score was {}",
        opportunity.risk_score
    );

    // Timestamp should be recent
    let time_diff = Utc::now() - opportunity.detected_at;
    assert!(time_diff.num_seconds() < 5);
}

// =============================================================================
// Test 2: Profitable Scenario
// =============================================================================

/// Tests a profitable arbitrage scenario where YES and NO both trade at 0.48.
/// Pair cost = 0.96, which should yield positive net profit after fees.
#[test]
fn test_profitable_scenario_both_at_048() {
    // Arrange: Both YES and NO at 0.48 (pair cost = 0.96)
    let mut yes_book = L2OrderBook::new("yes-token".to_string());
    yes_book.apply_snapshot(vec![], vec![(dec!(0.48), dec!(500))]);

    let mut no_book = L2OrderBook::new("no-token".to_string());
    no_book.apply_snapshot(vec![], vec![(dec!(0.48), dec!(500))]);

    let detector = ArbitrageDetector::default();

    // Act
    let opportunity = detector
        .detect("market-1", &yes_book, &no_book, dec!(100))
        .expect("Should detect profitable opportunity");

    // Assert
    assert_eq!(opportunity.pair_cost, dec!(0.96));

    // Gross profit = 1.00 - 0.96 = 0.04 (4 cents per pair)
    assert_eq!(opportunity.gross_profit_per_pair, dec!(0.04));

    // Expected fee = 0.01 * (2 - 0.96) = 0.0104
    assert_eq!(opportunity.expected_fee, dec!(0.0104));

    // Gas = 0.014
    assert_eq!(opportunity.gas_cost, dec!(0.014));

    // Net profit = 0.04 - 0.0104 - 0.014 = 0.0156
    assert_eq!(opportunity.net_profit_per_pair, dec!(0.0156));
    assert!(opportunity.net_profit_per_pair > Decimal::ZERO);

    // Total profit for 100 shares = 100 * 0.0156 = 1.56
    let total_profit = opportunity.recommended_size * opportunity.net_profit_per_pair;
    assert_eq!(total_profit, dec!(1.56));
}

// =============================================================================
// Test 3: Non-Profitable Scenario
// =============================================================================

/// Tests a non-profitable scenario where pair cost exceeds the threshold.
/// YES at 0.52 + NO at 0.52 = 1.04 pair cost (no arbitrage).
#[test]
fn test_non_profitable_scenario_both_at_052() {
    // Arrange: Both YES and NO at 0.52 (pair cost = 1.04)
    let mut yes_book = L2OrderBook::new("yes-token".to_string());
    yes_book.apply_snapshot(vec![], vec![(dec!(0.52), dec!(500))]);

    let mut no_book = L2OrderBook::new("no-token".to_string());
    no_book.apply_snapshot(vec![], vec![(dec!(0.52), dec!(500))]);

    let detector = ArbitrageDetector::default();

    // Act
    let opportunity = detector.detect("market-1", &yes_book, &no_book, dec!(100));

    // Assert: No opportunity should be returned
    assert!(
        opportunity.is_none(),
        "Should not detect opportunity when pair cost > 1.00"
    );
}

/// Tests scenario at the threshold boundary.
/// YES at 0.49 + NO at 0.49 = 0.98 pair cost (above 0.97 threshold).
#[test]
fn test_non_profitable_scenario_at_threshold_boundary() {
    // Arrange: Both at 0.49 (pair cost = 0.98, above 0.97 threshold)
    let mut yes_book = L2OrderBook::new("yes-token".to_string());
    yes_book.apply_snapshot(vec![], vec![(dec!(0.49), dec!(500))]);

    let mut no_book = L2OrderBook::new("no-token".to_string());
    no_book.apply_snapshot(vec![], vec![(dec!(0.49), dec!(500))]);

    let detector = ArbitrageDetector::default();

    // Act
    let opportunity = detector.detect("market-1", &yes_book, &no_book, dec!(100));

    // Assert: No opportunity (0.98 > 0.97 threshold)
    assert!(
        opportunity.is_none(),
        "Should not detect opportunity when pair cost (0.98) > threshold (0.97)"
    );
}

/// Tests a marginal non-profitable scenario due to fees eating into gross profit.
#[test]
fn test_non_profitable_due_to_fees() {
    // Arrange: Use a higher threshold to test fee impact
    let detector = ArbitrageDetector::new()
        .with_target_pair_cost(dec!(0.99)) // Allow higher pair costs
        .with_min_profit_threshold(dec!(0.01)); // Require 1 cent profit

    let mut yes_book = L2OrderBook::new("yes-token".to_string());
    yes_book.apply_snapshot(vec![], vec![(dec!(0.49), dec!(500))]);

    let mut no_book = L2OrderBook::new("no-token".to_string());
    no_book.apply_snapshot(vec![], vec![(dec!(0.49), dec!(500))]);

    // Act
    let opportunity = detector.detect("market-1", &yes_book, &no_book, dec!(100));

    // Assert: Should be rejected due to low net profit
    // Gross = 0.02, Fee = 0.0102, Gas = 0.014
    // Net = 0.02 - 0.0102 - 0.014 = -0.0042 (negative!)
    assert!(
        opportunity.is_none(),
        "Should not detect opportunity when fees exceed gross profit"
    );
}

// =============================================================================
// Test 4: Depth-Limited Scenario
// =============================================================================

/// Tests that worst_fill_price increases with larger order sizes
/// due to walking through multiple price levels.
#[test]
fn test_depth_limited_pricing_increases_with_size() {
    // Arrange: Multi-level order book with good prices at limited depth
    let mut yes_book = L2OrderBook::new("yes-token".to_string());
    yes_book.apply_snapshot(
        vec![],
        vec![
            (dec!(0.45), dec!(50)),  // Best level: only 50 available
            (dec!(0.48), dec!(100)), // Second level
            (dec!(0.52), dec!(200)), // Worst level
        ],
    );

    let mut no_book = L2OrderBook::new("no-token".to_string());
    no_book.apply_snapshot(
        vec![],
        vec![
            (dec!(0.45), dec!(50)),  // Best level: only 50 available
            (dec!(0.48), dec!(100)), // Second level
            (dec!(0.52), dec!(200)), // Worst level
        ],
    );

    let detector = ArbitrageDetector::default();

    // Act & Assert: Small order uses best prices
    let small_opp = detector
        .detect("market", &yes_book, &no_book, dec!(50))
        .expect("Should find opportunity for small order");

    assert_eq!(small_opp.yes_worst_fill, dec!(0.45));
    assert_eq!(small_opp.no_worst_fill, dec!(0.45));
    assert_eq!(small_opp.pair_cost, dec!(0.90)); // Best case

    // Medium order walks to second level
    let medium_opp = detector
        .detect("market", &yes_book, &no_book, dec!(100))
        .expect("Should find opportunity for medium order");

    assert_eq!(medium_opp.yes_worst_fill, dec!(0.48));
    assert_eq!(medium_opp.no_worst_fill, dec!(0.48));
    assert_eq!(medium_opp.pair_cost, dec!(0.96));

    // Large order walks to worst level - may exceed threshold
    let large_opp = detector.detect("market", &yes_book, &no_book, dec!(200));

    // At 200 shares: fills 50 @ 0.45 + 100 @ 0.48 + 50 @ 0.52
    // Worst price = 0.52, pair cost = 1.04 (exceeds threshold)
    assert!(
        large_opp.is_none(),
        "Large order should fail due to slippage exceeding threshold"
    );
}

/// Tests that opportunity disappears at larger sizes due to depth constraints.
#[test]
fn test_opportunity_disappears_at_larger_size() {
    // Arrange: Limited depth at good prices
    let mut yes_book = L2OrderBook::new("yes-token".to_string());
    yes_book.apply_snapshot(
        vec![],
        vec![
            (dec!(0.46), dec!(100)), // Good price, limited depth
            (dec!(0.55), dec!(500)), // Bad price
        ],
    );

    let mut no_book = L2OrderBook::new("no-token".to_string());
    no_book.apply_snapshot(
        vec![],
        vec![
            (dec!(0.46), dec!(100)), // Good price, limited depth
            (dec!(0.55), dec!(500)), // Bad price
        ],
    );

    let detector = ArbitrageDetector::default();

    // Act & Assert
    // At 100 shares: pair_cost = 0.92 (profitable)
    assert!(detector
        .detect("market", &yes_book, &no_book, dec!(100))
        .is_some());

    // At 150 shares: need to use second level (0.55)
    // Pair cost = 0.55 + 0.55 = 1.10 (not profitable)
    assert!(
        detector
            .detect("market", &yes_book, &no_book, dec!(150))
            .is_none(),
        "Opportunity should disappear when depth forces worse prices"
    );
}

/// Tests detect_at_sizes for optimal size finding.
#[test]
fn test_detect_at_multiple_sizes() {
    // Arrange: Multi-level book
    let mut yes_book = L2OrderBook::new("yes-token".to_string());
    yes_book.apply_snapshot(
        vec![],
        vec![
            (dec!(0.46), dec!(100)),
            (dec!(0.48), dec!(200)),
            (dec!(0.50), dec!(300)),
        ],
    );

    let mut no_book = L2OrderBook::new("no-token".to_string());
    no_book.apply_snapshot(
        vec![],
        vec![
            (dec!(0.46), dec!(100)),
            (dec!(0.48), dec!(200)),
            (dec!(0.50), dec!(300)),
        ],
    );

    let detector = ArbitrageDetector::default();

    // Act
    let sizes = vec![dec!(50), dec!(100), dec!(200), dec!(400), dec!(700)];
    let opportunities = detector.detect_at_sizes("market", &yes_book, &no_book, &sizes);

    // Assert: Should find opportunities at smaller sizes
    assert!(
        !opportunities.is_empty(),
        "Should find at least one opportunity"
    );

    // Verify sorted by total profit descending
    for window in opportunities.windows(2) {
        let profit_a = window[0].recommended_size * window[0].net_profit_per_pair;
        let profit_b = window[1].recommended_size * window[1].net_profit_per_pair;
        assert!(
            profit_a >= profit_b,
            "Results should be sorted by total profit descending"
        );
    }

    // Largest opportunities should have been filtered out due to slippage
    assert!(
        opportunities.len() < sizes.len(),
        "Some sizes should be unprofitable"
    );
}

// =============================================================================
// Test 5: Metrics Accumulation
// =============================================================================

/// Tests ArbitrageMetrics accumulation over multiple execution attempts.
#[test]
fn test_metrics_accumulation_basic() {
    let mut metrics = ArbitrageMetrics::new();

    // Simulate 50 windows with 60% detection rate
    for i in 0..50 {
        metrics.record_window(i % 5 < 3); // 60% detection
    }

    assert_eq!(metrics.windows_analyzed, 50);
    assert_eq!(metrics.opportunities_detected, 30);
    assert!((metrics.detection_rate - 0.6).abs() < 0.001);

    // Verify Wilson CI is reasonable for 60% with n=50
    let (ci_lower, ci_upper) = metrics.detection_rate_wilson_ci;
    assert!(ci_lower > 0.45 && ci_lower < 0.55);
    assert!(ci_upper > 0.70 && ci_upper < 0.80);
}

/// Tests Wilson CI calculations directly.
#[test]
fn test_wilson_ci_calculations() {
    // 70% success rate with 50 samples
    let (lower, upper) = wilson_ci(35, 50, 1.96);

    // Expected: approximately (0.56, 0.81) for 95% CI
    assert!(lower > 0.55 && lower < 0.60, "Lower CI was {}", lower);
    assert!(upper > 0.79 && upper < 0.83, "Upper CI was {}", upper);

    // 80% success rate with 100 samples (narrower CI)
    let (lower2, upper2) = wilson_ci(80, 100, 1.96);
    assert!(lower2 > 0.70 && lower2 < 0.75, "Lower CI was {}", lower2);
    assert!(upper2 > 0.85 && upper2 < 0.90, "Upper CI was {}", upper2);

    // Edge case: 0 successes
    let (lower_zero, upper_zero) = wilson_ci(0, 10, 1.96);
    assert!(lower_zero >= 0.0);
    assert!(upper_zero < 0.35);

    // Edge case: all successes
    let (lower_all, upper_all) = wilson_ci(10, 10, 1.96);
    assert!(lower_all > 0.65);
    assert!((upper_all - 1.0_f64).abs() < 0.02);
}

/// Tests execution recording and fill rate calculation.
#[test]
fn test_metrics_execution_recording() {
    let mut metrics = ArbitrageMetrics::new();

    // Simulate execution attempts: 35 successes, 10 failures, 5 partial fills
    for _ in 0..35 {
        metrics.record_execution(true, false);
    }
    for _ in 0..10 {
        metrics.record_execution(false, false);
    }
    for _ in 0..5 {
        metrics.record_execution(false, true); // Partial fill counts as failure
    }

    assert_eq!(metrics.attempts, 50);
    assert_eq!(metrics.successful_pairs, 35);
    assert_eq!(metrics.partial_fills, 5);
    assert!((metrics.fill_rate - 0.70).abs() < 0.001);

    // Verify Wilson CI
    let (ci_lower, ci_upper) = metrics.fill_rate_wilson_ci;
    assert!(ci_lower > 0.55, "CI lower was {}", ci_lower);
    assert!(ci_upper < 0.85, "CI upper was {}", ci_upper);
}

/// Tests Go/No-Go gate criteria.
#[test]
fn test_go_no_go_gates() {
    let mut metrics = ArbitrageMetrics::new();

    // Scenario 1: Not enough samples
    for _ in 0..30 {
        metrics.record_execution(true, false);
    }
    assert!(
        !metrics.fill_rate_acceptable(),
        "Should fail: insufficient samples"
    );

    // Add more to reach MIN_SAMPLE_SIZE (41)
    for _ in 0..11 {
        metrics.record_execution(true, false);
    }
    assert_eq!(metrics.attempts, 41);
    assert!(metrics.attempts >= MIN_SAMPLE_SIZE);

    // With 41/41 successes, CI lower should be > 60%
    assert!(
        metrics.fill_rate_acceptable(),
        "Should pass with high fill rate"
    );

    // Scenario 2: High samples but low fill rate
    let mut low_rate_metrics = ArbitrageMetrics::new();
    for _ in 0..25 {
        low_rate_metrics.record_execution(true, false);
    }
    for _ in 0..25 {
        low_rate_metrics.record_execution(false, false);
    }
    // 50% fill rate with n=50 - CI includes values below 60%
    assert!(
        !low_rate_metrics.fill_rate_acceptable(),
        "Should fail: fill rate CI too wide"
    );
}

/// Tests profit significance calculation.
#[test]
fn test_profit_significance() {
    let mut metrics = ArbitrageMetrics::new();

    // Need minimum sample size
    metrics.attempts = 50;

    // Simulate consistent positive profits
    let profits: Vec<Decimal> = (0..50)
        .map(|_| dec!(0.015) + dec!(0.002)) // ~$0.017 per pair
        .collect();

    metrics.update_profit_statistics(&profits);

    // With consistent positive profits, p-value should be very low
    assert!(
        metrics.profit_p_value < 0.05,
        "p-value was {}",
        metrics.profit_p_value
    );
    assert!(metrics.profit_significant());
}

/// Tests production readiness criteria.
#[test]
fn test_production_readiness() {
    let mut metrics = ArbitrageMetrics::new();

    // Set up all passing criteria
    // 1. High fill rate (80% with 50 attempts)
    for _ in 0..40 {
        metrics.record_execution(true, false);
    }
    for _ in 0..10 {
        metrics.record_execution(false, false);
    }

    // 2. Significant profit (p < 0.10)
    let profits: Vec<Decimal> = (0..40).map(|_| dec!(0.02)).collect();
    metrics.update_profit_statistics(&profits);

    // 3. Low imbalance
    metrics.max_imbalance = dec!(30);

    // 4. Positive P&L
    metrics.total_pnl = dec!(50);

    // Should be production ready
    assert!(metrics.fill_rate_acceptable());
    assert!(metrics.profit_significant());
    assert!(metrics.production_ready());
}

/// Tests production readiness fails on various criteria.
#[test]
fn test_production_readiness_failures() {
    // Failure case 1: High imbalance
    let mut high_imbalance = ArbitrageMetrics::new();
    for _ in 0..45 {
        high_imbalance.record_execution(true, false);
    }
    high_imbalance.profit_p_value = 0.05;
    high_imbalance.max_imbalance = dec!(60); // > 50 limit
    high_imbalance.total_pnl = dec!(100);

    assert!(
        !high_imbalance.production_ready(),
        "Should fail: high imbalance"
    );

    // Failure case 2: Negative P&L
    let mut negative_pnl = ArbitrageMetrics::new();
    for _ in 0..45 {
        negative_pnl.record_execution(true, false);
    }
    negative_pnl.profit_p_value = 0.05;
    negative_pnl.max_imbalance = dec!(30);
    negative_pnl.total_pnl = dec!(-10);

    assert!(
        !negative_pnl.production_ready(),
        "Should fail: negative P&L"
    );
}

/// Tests validation summary generation.
#[test]
fn test_validation_summary() {
    let mut metrics = ArbitrageMetrics::new();

    for _ in 0..35 {
        metrics.record_execution(true, false);
    }
    for _ in 0..15 {
        metrics.record_execution(false, false);
    }

    metrics.mean_net_profit_per_pair = dec!(0.018);
    metrics.profit_p_value = 0.03;
    metrics.max_imbalance = dec!(25);
    metrics.total_pnl = dec!(72);

    let summary = metrics.validation_summary();

    assert_eq!(summary.attempts, 50);
    assert_eq!(summary.min_required, MIN_SAMPLE_SIZE);
    assert!((summary.fill_rate - 0.70).abs() < 0.001);
    assert!(summary.fill_rate_ci_lower > 0.0);
    assert!(summary.fill_rate_ci_upper < 1.0);
    assert_eq!(summary.mean_profit, dec!(0.018));
    assert!((summary.profit_p_value - 0.03).abs() < 0.001);
    assert!(summary.profit_significant);
    assert_eq!(summary.max_imbalance, dec!(25));
    assert!(summary.imbalance_acceptable);
    assert_eq!(summary.total_pnl, dec!(72));
}

// =============================================================================
// Test 6: Position Tracking
// =============================================================================

/// Tests ArbitragePosition creation and basic calculations.
#[test]
fn test_position_creation_and_calculations() {
    // Create a balanced position with 100 YES and 100 NO
    let position = create_test_position(
        dec!(100), // yes_shares
        dec!(48),  // yes_cost (avg price 0.48)
        dec!(100), // no_shares
        dec!(48),  // no_cost (avg price 0.48)
    );

    // Pair cost = (48 + 48) / 100 = 0.96
    assert_eq!(position.calculate_pair_cost(), dec!(0.96));

    // Guaranteed payout = min(100, 100) = 100
    assert_eq!(position.calculate_guaranteed_payout(), dec!(100));

    // Guaranteed profit = payout - total_cost = 100 - 96 = 4
    assert_eq!(position.guaranteed_profit(), dec!(4));

    // Imbalance = 100 - 100 = 0
    assert_eq!(position.calculate_imbalance(), Decimal::ZERO);
    assert!(position.is_balanced(dec!(1)));
}

/// Tests position with YES/NO imbalance.
#[test]
fn test_position_with_imbalance() {
    // Create an imbalanced position: 110 YES, 90 NO
    let position = create_test_position(
        dec!(110),  // yes_shares
        dec!(52.8), // yes_cost (avg price 0.48)
        dec!(90),   // no_shares
        dec!(43.2), // no_cost (avg price 0.48)
    );

    // Imbalance = 110 - 90 = 20
    assert_eq!(position.calculate_imbalance(), dec!(20));
    assert!(!position.is_balanced(dec!(10)));
    assert!(position.is_balanced(dec!(20)));

    // Imbalance ratio = 20 / 110 = 0.1818...
    let ratio = position.imbalance_ratio();
    assert!(ratio > dec!(0.18) && ratio < dec!(0.19));

    // Guaranteed payout = min(110, 90) = 90
    assert_eq!(position.calculate_guaranteed_payout(), dec!(90));

    // Pair cost based on balanced portion = (52.8 + 43.2) / 90 = 1.0666...
    let pair_cost = position.calculate_pair_cost();
    assert!(pair_cost > dec!(1.06) && pair_cost < dec!(1.07));
}

/// Tests adding fills to a position.
#[test]
fn test_position_fill_accumulation() {
    // Start with empty position
    let mut yes_shares = Decimal::ZERO;
    let mut yes_cost = Decimal::ZERO;
    let mut no_shares = Decimal::ZERO;
    let mut no_cost = Decimal::ZERO;

    // First YES fill: 50 shares @ 0.47
    yes_shares += dec!(50);
    yes_cost += dec!(50) * dec!(0.47);

    // First NO fill: 50 shares @ 0.48
    no_shares += dec!(50);
    no_cost += dec!(50) * dec!(0.48);

    let position1 = create_test_position(yes_shares, yes_cost, no_shares, no_cost);
    assert_eq!(position1.calculate_imbalance(), Decimal::ZERO);
    assert_eq!(position1.calculate_guaranteed_payout(), dec!(50));

    // Second YES fill: 30 shares @ 0.46
    yes_shares += dec!(30);
    yes_cost += dec!(30) * dec!(0.46);

    let position2 = create_test_position(yes_shares, yes_cost, no_shares, no_cost);
    assert_eq!(position2.calculate_imbalance(), dec!(30)); // 80 YES - 50 NO
    assert_eq!(position2.calculate_guaranteed_payout(), dec!(50)); // Still limited by NO

    // Second NO fill: 30 shares @ 0.49
    no_shares += dec!(30);
    no_cost += dec!(30) * dec!(0.49);

    let position3 = create_test_position(yes_shares, yes_cost, no_shares, no_cost);
    assert_eq!(position3.calculate_imbalance(), Decimal::ZERO); // Back to balanced
    assert_eq!(position3.calculate_guaranteed_payout(), dec!(80));

    // Calculate final pair cost
    // YES: 50 * 0.47 + 30 * 0.46 = 23.5 + 13.8 = 37.3
    // NO: 50 * 0.48 + 30 * 0.49 = 24 + 14.7 = 38.7
    // Total: 76, Shares: 80, Pair cost: 0.95
    let pair_cost = position3.calculate_pair_cost();
    assert_eq!(pair_cost, dec!(0.95));
}

/// Tests position status transitions.
#[test]
fn test_position_status_tracking() {
    let mut position = create_test_position(dec!(100), dec!(48), dec!(100), dec!(48));

    // Initial status should be Building
    assert_eq!(position.status, PositionStatus::Building);

    // Transition to Complete when balanced
    position.status = PositionStatus::Complete;
    assert_eq!(position.status, PositionStatus::Complete);

    // Transition to Settling when market closes
    position.status = PositionStatus::Settling;
    assert_eq!(position.status, PositionStatus::Settling);

    // Transition to Settled after payout
    position.status = PositionStatus::Settled;
    assert_eq!(position.status, PositionStatus::Settled);
}

/// Tests position guaranteed profit calculation for different pair costs.
#[test]
fn test_position_profit_calculation_scenarios() {
    // Scenario 1: Excellent arbitrage (pair cost 0.94)
    let excellent = create_test_position(
        dec!(100),
        dec!(47), // YES @ 0.47
        dec!(100),
        dec!(47), // NO @ 0.47
    );
    assert_eq!(excellent.guaranteed_profit(), dec!(6)); // 100 - 94 = 6

    // Scenario 2: Marginal arbitrage (pair cost 0.98)
    let marginal = create_test_position(
        dec!(100),
        dec!(49), // YES @ 0.49
        dec!(100),
        dec!(49), // NO @ 0.49
    );
    assert_eq!(marginal.guaranteed_profit(), dec!(2)); // 100 - 98 = 2

    // Scenario 3: No arbitrage (pair cost 1.02)
    let loss = create_test_position(
        dec!(100),
        dec!(51), // YES @ 0.51
        dec!(100),
        dec!(51), // NO @ 0.51
    );
    assert_eq!(loss.guaranteed_profit(), dec!(-2)); // 100 - 102 = -2
}

// =============================================================================
// Integration Scenario Tests
// =============================================================================

/// End-to-end test simulating a realistic trading session.
#[test]
fn test_realistic_trading_session_simulation() {
    let mut metrics = ArbitrageMetrics::new();
    let detector = ArbitrageDetector::default();

    // Simulate 100 15-minute windows
    let mut total_profit = Decimal::ZERO;
    let mut profits_list = Vec::new();

    for window in 0..100 {
        // Create order books with varying prices
        let yes_price = dec!(0.46) + Decimal::from(window % 5) * dec!(0.01);
        let no_price = dec!(0.46) + Decimal::from((window + 2) % 5) * dec!(0.01);

        let yes_book = create_realistic_yes_orderbook(yes_price, dec!(200));
        let no_book = create_realistic_no_orderbook(no_price, dec!(200));

        let opportunity = detector.detect("market", &yes_book, &no_book, dec!(100));

        if let Some(opp) = opportunity {
            metrics.record_window(true);

            // Simulate execution (80% success rate)
            let success = window % 5 != 0;
            metrics.record_execution(success, !success && window % 10 == 0);

            if success {
                // Calculate total profit for this trade (per-pair * size)
                let trade_profit = opp.net_profit_per_pair * opp.recommended_size;
                total_profit += trade_profit;
                profits_list.push(opp.net_profit_per_pair);

                // record_profit expects total net_profit for this trade, not per-pair
                metrics.record_profit(
                    trade_profit,
                    opp.total_investment,
                    opp.guaranteed_payout,
                    (opp.expected_fee + opp.gas_cost) * opp.recommended_size,
                );
            }
        } else {
            metrics.record_window(false);
        }
    }

    // Update profit statistics
    if !profits_list.is_empty() {
        metrics.update_profit_statistics(&profits_list);
    }

    // Verify metrics accumulated correctly
    assert_eq!(metrics.windows_analyzed, 100);
    assert!(metrics.opportunities_detected > 0);
    assert!(metrics.attempts > 0);

    // Verify detection rate makes sense (depends on price distribution)
    assert!(metrics.detection_rate > 0.0 && metrics.detection_rate < 1.0);

    // Verify total P&L
    assert_eq!(metrics.total_pnl, total_profit);
}

/// Tests break-even pair cost calculation.
#[test]
fn test_break_even_pair_cost() {
    let detector = ArbitrageDetector::default();
    let break_even = detector.break_even_pair_cost();

    // With default gas_cost = 0.007
    // break_even = (0.98 - 0.014) / 0.99 = 0.966 / 0.99 ~ 0.9757
    assert!(
        break_even > dec!(0.975) && break_even < dec!(0.976),
        "Break-even was {}",
        break_even
    );

    // At break-even, should NOT be profitable
    assert!(!detector.is_pair_cost_profitable(break_even));

    // Just below break-even should be profitable
    assert!(detector.is_pair_cost_profitable(break_even - dec!(0.01)));
}

/// Tests detector with custom configuration.
#[test]
fn test_detector_custom_configuration() {
    // Conservative detector
    let conservative = ArbitrageDetector::new()
        .with_target_pair_cost(dec!(0.95))
        .with_min_profit_threshold(dec!(0.02))
        .with_max_position_size(dec!(500))
        .with_gas_cost(dec!(0.01));

    let mut yes_book = L2OrderBook::new("yes".to_string());
    yes_book.apply_snapshot(vec![], vec![(dec!(0.47), dec!(1000))]);

    let mut no_book = L2OrderBook::new("no".to_string());
    no_book.apply_snapshot(vec![], vec![(dec!(0.47), dec!(1000))]);

    // Pair cost = 0.94, should pass threshold (0.95)
    let opp = conservative
        .detect("market", &yes_book, &no_book, dec!(1000))
        .expect("Should detect with conservative settings");

    // Size should be capped at 500
    assert_eq!(opp.recommended_size, dec!(500));

    // Gas cost should be 0.02 (0.01 * 2)
    assert_eq!(opp.gas_cost, dec!(0.02));
}
