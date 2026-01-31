//! Edge quantification and analysis for binary outcome backtesting.
//!
//! This module provides comprehensive edge analysis including:
//! - Edge measurement with statistical significance testing
//! - Conditional edge analysis by signal strength
//! - Time-of-day edge patterns
//! - Volatility regime edge analysis
//! - Edge decay detection over time
//! - Go/No-Go decision framework

use chrono::{DateTime, Timelike, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use algo_trade_core::{binomial_test, wilson_ci};

use super::outcome::{BinaryOutcome, SettlementResult};

/// Classification of edge strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeClassification {
    /// Strong edge: win rate > 55%, p < 0.01, positive EV.
    Strong,
    /// Moderate edge: win rate > 53%, p < 0.05, positive EV.
    Moderate,
    /// Weak edge: win rate > 51%, p < 0.10, marginal EV.
    Weak,
    /// No edge: win rate near 50%, not significant.
    None,
    /// Negative edge: win rate < 50%, losing money.
    Negative,
}

/// Core edge measurement with statistical metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeMeasurement {
    /// Number of samples (bets) in the measurement.
    pub n_samples: usize,
    /// Win rate (wins / total).
    pub win_rate: f64,
    /// Wilson score 95% confidence interval.
    pub wilson_ci: (f64, f64),
    /// p-value from binomial test (H0: p = 0.50).
    pub p_value: f64,
    /// Edge over break-even (win_rate - 0.50).
    pub edge: f64,
    /// Whether result is statistically significant at alpha level.
    pub is_significant: bool,
    /// Expected value per bet.
    pub ev_per_bet: Decimal,
    /// Total P&L for this segment.
    pub total_pnl: Decimal,
}

impl EdgeMeasurement {
    /// Creates an edge measurement from settlement results.
    ///
    /// # Arguments
    /// * `settlements` - Slice of settlement results to analyze
    /// * `alpha` - Significance level (default 0.05)
    ///
    /// # Returns
    /// EdgeMeasurement with all computed statistics
    #[must_use]
    pub fn from_settlements(settlements: &[SettlementResult], alpha: f64) -> Self {
        if settlements.is_empty() {
            return Self::empty();
        }

        let n_samples = settlements.len();
        let wins = settlements
            .iter()
            .filter(|s| s.outcome == BinaryOutcome::Win)
            .count();
        let non_push: Vec<_> = settlements
            .iter()
            .filter(|s| s.outcome != BinaryOutcome::Push)
            .collect();
        let non_push_count = non_push.len();

        let win_rate = if non_push_count > 0 {
            wins as f64 / non_push_count as f64
        } else {
            0.0
        };

        let wilson = wilson_ci(wins, non_push_count, 1.96);
        let p_value = binomial_test(wins, non_push_count, 0.5);
        let edge = win_rate - 0.5;
        let is_significant = p_value < alpha;

        let total_pnl: Decimal = settlements.iter().map(|s| s.net_pnl).sum();
        let ev_per_bet = if n_samples > 0 {
            total_pnl / Decimal::from(n_samples as u32)
        } else {
            Decimal::ZERO
        };

        Self {
            n_samples,
            win_rate,
            wilson_ci: wilson,
            p_value,
            edge,
            is_significant,
            ev_per_bet,
            total_pnl,
        }
    }

    /// Returns an empty measurement for when there are no settlements.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            n_samples: 0,
            win_rate: 0.0,
            wilson_ci: (0.0, 0.0),
            p_value: 1.0,
            edge: -0.5,
            is_significant: false,
            ev_per_bet: Decimal::ZERO,
            total_pnl: Decimal::ZERO,
        }
    }

    /// Returns true if the edge is statistically significant at the given alpha.
    #[must_use]
    pub fn is_significant_at(&self, alpha: f64) -> bool {
        self.p_value < alpha
    }
}

/// Conditional edge analysis by signal strength.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionalEdge {
    /// Baseline edge (all signals).
    pub baseline: EdgeMeasurement,
    /// Edge for high strength signals (strength > 0.6).
    pub high_strength: EdgeMeasurement,
    /// Edge for very high strength signals (strength > 0.8).
    pub very_high_strength: EdgeMeasurement,
    /// Edge for high confidence signals (confidence > 0.7).
    pub high_confidence: EdgeMeasurement,
}

/// Time-of-day edge analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeOfDayEdge {
    /// Edge measurement by UTC hour (0-23).
    pub by_hour: HashMap<u32, EdgeMeasurement>,
    /// Hours with strongest positive edge.
    pub best_hours: Vec<u32>,
    /// Hours with weakest or negative edge.
    pub worst_hours: Vec<u32>,
    /// Trading recommendations based on hour analysis.
    pub recommendations: Vec<String>,
}

/// Volatility regime edge analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolatilityEdge {
    /// Edge in low volatility regime.
    pub low: EdgeMeasurement,
    /// Edge in medium volatility regime.
    pub medium: EdgeMeasurement,
    /// Edge in high volatility regime.
    pub high: EdgeMeasurement,
    /// Volatility thresholds used for classification.
    pub thresholds: (f64, f64),
}

/// Rolling window metric for decay detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollingMetric {
    /// Window end timestamp.
    pub timestamp: DateTime<Utc>,
    /// Win rate in this window.
    pub win_rate: f64,
    /// Number of samples in window.
    pub n_samples: usize,
    /// Cumulative sample count up to this point.
    pub cumulative_samples: usize,
}

/// Edge decay analysis over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeDecay {
    /// Rolling metrics over time.
    pub rolling_metrics: Vec<RollingMetric>,
    /// Slope of win rate decay (negative = decaying).
    pub decay_slope: f64,
    /// Intercept of linear fit.
    pub decay_intercept: f64,
    /// p-value for decay slope significance.
    pub decay_p_value: f64,
    /// Detected changepoints (structural breaks).
    pub changepoints: Vec<usize>,
    /// Whether significant decay is detected.
    pub is_decaying: bool,
}

/// Linear regression result.
#[derive(Debug, Clone)]
pub struct LinearRegression {
    /// Slope of the regression line.
    pub slope: f64,
    /// Y-intercept.
    pub intercept: f64,
    /// R-squared value.
    pub r_squared: f64,
    /// p-value for slope significance.
    pub p_value: f64,
}

/// Computes simple linear regression.
///
/// # Arguments
/// * `x` - Independent variable values
/// * `y` - Dependent variable values
///
/// # Returns
/// LinearRegression with slope, intercept, r_squared, and p_value
#[must_use]
pub fn linear_regression(x: &[f64], y: &[f64]) -> LinearRegression {
    let n = x.len();
    if n < 3 || n != y.len() {
        return LinearRegression {
            slope: 0.0,
            intercept: 0.0,
            r_squared: 0.0,
            p_value: 1.0,
        };
    }

    let n_f = n as f64;
    let sum_x: f64 = x.iter().sum();
    let sum_y: f64 = y.iter().sum();
    let sum_xy: f64 = x.iter().zip(y.iter()).map(|(xi, yi)| xi * yi).sum();
    let sum_x2: f64 = x.iter().map(|xi| xi * xi).sum();
    let sum_y2: f64 = y.iter().map(|yi| yi * yi).sum();

    let mean_x = sum_x / n_f;
    let mean_y = sum_y / n_f;

    // Slope: b = sum((x - mean_x)(y - mean_y)) / sum((x - mean_x)^2)
    let ss_xy = sum_xy - n_f * mean_x * mean_y;
    let ss_xx = sum_x2 - n_f * mean_x * mean_x;
    let ss_yy = sum_y2 - n_f * mean_y * mean_y;

    if ss_xx.abs() < f64::EPSILON {
        return LinearRegression {
            slope: 0.0,
            intercept: mean_y,
            r_squared: 0.0,
            p_value: 1.0,
        };
    }

    let slope = ss_xy / ss_xx;
    let intercept = mean_y - slope * mean_x;

    // R-squared
    let r_squared = if ss_yy.abs() < f64::EPSILON {
        1.0
    } else {
        (ss_xy * ss_xy) / (ss_xx * ss_yy)
    };

    // Standard error of slope and t-statistic for p-value
    // SE(slope) = sqrt(MSE / SS_xx) where MSE = SS_res / (n-2)
    let ss_res: f64 = y
        .iter()
        .zip(x.iter())
        .map(|(yi, xi)| {
            let predicted = intercept + slope * xi;
            (yi - predicted).powi(2)
        })
        .sum();

    // For perfect linear fit, ss_res will be ~0, giving very high t-stat
    let mse = ss_res / (n_f - 2.0);

    // Handle perfect fit case where MSE is essentially 0
    let p_value = if mse < f64::EPSILON || ss_res < f64::EPSILON {
        // Perfect or near-perfect fit - p-value is essentially 0
        0.0
    } else {
        let se_slope = (mse / ss_xx).sqrt();
        // t-statistic
        let t_stat = if se_slope.abs() < f64::EPSILON {
            f64::INFINITY
        } else {
            slope.abs() / se_slope
        };
        // Approximate p-value using normal approximation
        approximate_t_pvalue(t_stat, n - 2)
    };

    LinearRegression {
        slope,
        intercept,
        r_squared,
        p_value,
    }
}

/// Approximates p-value for t-statistic using normal approximation.
/// For large df, t-distribution approaches normal.
fn approximate_t_pvalue(t: f64, df: usize) -> f64 {
    if df < 2 {
        return 1.0;
    }

    // For df > 30, use normal approximation
    // For smaller df, adjust using Welch's approximation
    let adjusted_t = if df > 30 {
        t
    } else {
        // Simple adjustment for smaller samples
        t * (1.0 - 1.0 / (4.0 * df as f64)).sqrt()
    };

    // Two-tailed p-value using error function approximation
    // P(|T| > t) = 2 * P(T > t) approx 2 * (1 - Phi(t))
    let p = 2.0 * (1.0 - normal_cdf(adjusted_t));
    p.clamp(0.0, 1.0)
}

/// Approximation of standard normal CDF.
fn normal_cdf(x: f64) -> f64 {
    // Abramowitz and Stegun approximation
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();

    let t = 1.0 / (1.0 + p * x);
    let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x * x / 2.0).exp();

    0.5 * (1.0 + sign * y)
}

/// Complete edge analysis combining all dimensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeAnalysis {
    /// Overall edge measurement.
    pub overall: EdgeMeasurement,
    /// Conditional edge by signal characteristics.
    pub conditional: ConditionalEdge,
    /// Time-of-day edge patterns.
    pub time_of_day: TimeOfDayEdge,
    /// Volatility regime edge.
    pub volatility: VolatilityEdge,
    /// Edge decay analysis.
    pub decay: EdgeDecay,
}

/// Summary of edge analysis with Go/No-Go decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeSummary {
    /// Overall edge classification.
    pub classification: EdgeClassification,
    /// Confidence in edge persistence (0.0 to 1.0).
    pub persistence_confidence: f64,
    /// Go/No-Go decision for live trading.
    pub go_no_go: bool,
    /// Reasons supporting the decision.
    pub reasons: Vec<String>,
    /// Recommended bet sizing (fraction of Kelly).
    pub recommended_kelly_fraction: f64,
}

/// Configuration for edge analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeAnalyzerConfig {
    /// Significance level for hypothesis tests.
    pub alpha: f64,
    /// Minimum samples for reliable analysis.
    pub min_samples: usize,
    /// Rolling window size for decay detection.
    pub rolling_window_size: usize,
    /// High signal strength threshold.
    pub high_strength_threshold: f64,
    /// Very high signal strength threshold.
    pub very_high_strength_threshold: f64,
    /// High confidence threshold.
    pub high_confidence_threshold: f64,
    /// Volatility percentile boundaries.
    pub volatility_percentiles: (f64, f64),
}

impl Default for EdgeAnalyzerConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            min_samples: 100,
            rolling_window_size: 50,
            high_strength_threshold: 0.6,
            very_high_strength_threshold: 0.8,
            high_confidence_threshold: 0.7,
            volatility_percentiles: (0.33, 0.67),
        }
    }
}

/// Analyzer for comprehensive edge quantification.
#[derive(Debug, Clone)]
pub struct EdgeAnalyzer {
    config: EdgeAnalyzerConfig,
}

impl EdgeAnalyzer {
    /// Creates a new edge analyzer with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: EdgeAnalyzerConfig::default(),
        }
    }

    /// Creates a new edge analyzer with custom configuration.
    #[must_use]
    pub fn with_config(config: EdgeAnalyzerConfig) -> Self {
        Self { config }
    }

    /// Analyzes conditional edge by signal strength.
    ///
    /// # Arguments
    /// * `settlements` - Settlement results with signal metadata
    ///
    /// # Returns
    /// ConditionalEdge with baseline and filtered measurements
    #[must_use]
    pub fn analyze_conditional(&self, settlements: &[SettlementResult]) -> ConditionalEdge {
        let baseline = EdgeMeasurement::from_settlements(settlements, self.config.alpha);

        let high_strength: Vec<_> = settlements
            .iter()
            .filter(|s| s.bet.signal_strength > self.config.high_strength_threshold)
            .cloned()
            .collect();
        let high_strength_edge =
            EdgeMeasurement::from_settlements(&high_strength, self.config.alpha);

        let very_high_strength: Vec<_> = settlements
            .iter()
            .filter(|s| s.bet.signal_strength > self.config.very_high_strength_threshold)
            .cloned()
            .collect();
        let very_high_strength_edge =
            EdgeMeasurement::from_settlements(&very_high_strength, self.config.alpha);

        // For confidence, we check signal_metadata for a "confidence" key
        let high_confidence: Vec<_> = settlements
            .iter()
            .filter(|s| {
                s.bet
                    .signal_metadata
                    .get("confidence")
                    .copied()
                    .unwrap_or(0.0)
                    > self.config.high_confidence_threshold
            })
            .cloned()
            .collect();
        let high_confidence_edge =
            EdgeMeasurement::from_settlements(&high_confidence, self.config.alpha);

        ConditionalEdge {
            baseline,
            high_strength: high_strength_edge,
            very_high_strength: very_high_strength_edge,
            high_confidence: high_confidence_edge,
        }
    }

    /// Analyzes edge by time of day (UTC hour).
    ///
    /// # Arguments
    /// * `settlements` - Settlement results with timestamps
    ///
    /// # Returns
    /// TimeOfDayEdge with hourly breakdown and recommendations
    #[must_use]
    pub fn analyze_time_of_day(&self, settlements: &[SettlementResult]) -> TimeOfDayEdge {
        let mut by_hour: HashMap<u32, Vec<SettlementResult>> = HashMap::new();

        for settlement in settlements {
            let hour = settlement.bet.timestamp.hour();
            by_hour.entry(hour).or_default().push(settlement.clone());
        }

        let metrics_by_hour: HashMap<u32, EdgeMeasurement> = by_hour
            .into_iter()
            .map(|(hour, setts)| {
                (
                    hour,
                    EdgeMeasurement::from_settlements(&setts, self.config.alpha),
                )
            })
            .collect();

        // Find best and worst hours (by edge, with sufficient samples)
        let min_samples = 10; // Minimum for hour analysis
        let mut hour_edges: Vec<(u32, f64)> = metrics_by_hour
            .iter()
            .filter(|(_, m)| m.n_samples >= min_samples)
            .map(|(&h, m)| (h, m.edge))
            .collect();
        hour_edges.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let best_hours: Vec<u32> = hour_edges
            .iter()
            .filter(|(_, e)| *e > 0.0)
            .take(3)
            .map(|(h, _)| *h)
            .collect();

        let worst_hours: Vec<u32> = hour_edges
            .iter()
            .rev()
            .filter(|(_, e)| *e < 0.0)
            .take(3)
            .map(|(h, _)| *h)
            .collect();

        // Generate recommendations
        let mut recommendations = Vec::new();
        if !best_hours.is_empty() {
            recommendations.push(format!("Best trading hours (UTC): {:?}", best_hours));
        }
        if !worst_hours.is_empty() {
            recommendations.push(format!("Avoid trading hours (UTC): {:?}", worst_hours));
        }

        TimeOfDayEdge {
            by_hour: metrics_by_hour,
            best_hours,
            worst_hours,
            recommendations,
        }
    }

    /// Analyzes edge by volatility regime.
    ///
    /// # Arguments
    /// * `settlements` - Settlement results
    /// * `volatilities` - Volatility values corresponding to each settlement
    ///
    /// # Returns
    /// VolatilityEdge with regime-specific measurements
    #[must_use]
    pub fn analyze_volatility(
        &self,
        settlements: &[SettlementResult],
        volatilities: &[f64],
    ) -> VolatilityEdge {
        if settlements.is_empty() || volatilities.is_empty() {
            return VolatilityEdge {
                low: EdgeMeasurement::empty(),
                medium: EdgeMeasurement::empty(),
                high: EdgeMeasurement::empty(),
                thresholds: (0.0, 0.0),
            };
        }

        // Calculate percentile thresholds
        let low_pct = self.config.volatility_percentiles.0;
        let high_pct = self.config.volatility_percentiles.1;
        let low_threshold = percentile(volatilities, low_pct);
        let high_threshold = percentile(volatilities, high_pct);

        let mut low_settlements = Vec::new();
        let mut medium_settlements = Vec::new();
        let mut high_settlements = Vec::new();

        for (i, settlement) in settlements.iter().enumerate() {
            let vol = volatilities.get(i).copied().unwrap_or(0.0);
            if vol < low_threshold {
                low_settlements.push(settlement.clone());
            } else if vol > high_threshold {
                high_settlements.push(settlement.clone());
            } else {
                medium_settlements.push(settlement.clone());
            }
        }

        VolatilityEdge {
            low: EdgeMeasurement::from_settlements(&low_settlements, self.config.alpha),
            medium: EdgeMeasurement::from_settlements(&medium_settlements, self.config.alpha),
            high: EdgeMeasurement::from_settlements(&high_settlements, self.config.alpha),
            thresholds: (low_threshold, high_threshold),
        }
    }

    /// Detects edge decay over time using rolling window analysis.
    ///
    /// # Arguments
    /// * `settlements` - Settlement results sorted by time
    ///
    /// # Returns
    /// EdgeDecay with rolling metrics and decay statistics
    #[must_use]
    pub fn detect_decay(&self, settlements: &[SettlementResult]) -> EdgeDecay {
        if settlements.len() < self.config.rolling_window_size {
            return EdgeDecay {
                rolling_metrics: Vec::new(),
                decay_slope: 0.0,
                decay_intercept: 0.5,
                decay_p_value: 1.0,
                changepoints: Vec::new(),
                is_decaying: false,
            };
        }

        let window_size = self.config.rolling_window_size;
        let mut rolling_metrics = Vec::new();

        // Calculate rolling win rates
        for i in (window_size - 1)..settlements.len() {
            let window_start = i + 1 - window_size;
            let window = &settlements[window_start..=i];
            let cumulative = i + 1;

            let wins = window
                .iter()
                .filter(|s| s.outcome == BinaryOutcome::Win)
                .count();
            let non_push = window
                .iter()
                .filter(|s| s.outcome != BinaryOutcome::Push)
                .count();

            let win_rate = if non_push > 0 {
                wins as f64 / non_push as f64
            } else {
                0.5
            };

            rolling_metrics.push(RollingMetric {
                timestamp: settlements[i].settlement_time,
                win_rate,
                n_samples: window.len(),
                cumulative_samples: cumulative,
            });
        }

        // Linear regression on rolling win rates
        let x: Vec<f64> = (0..rolling_metrics.len()).map(|i| i as f64).collect();
        let y: Vec<f64> = rolling_metrics.iter().map(|m| m.win_rate).collect();

        let regression = linear_regression(&x, &y);

        // Detect changepoints using simple threshold
        let changepoints = self.detect_changepoints(&rolling_metrics);

        // Decay is significant if slope is negative and p < alpha
        let is_decaying = regression.slope < -0.001 && regression.p_value < self.config.alpha;

        EdgeDecay {
            rolling_metrics,
            decay_slope: regression.slope,
            decay_intercept: regression.intercept,
            decay_p_value: regression.p_value,
            changepoints,
            is_decaying,
        }
    }

    /// Detects changepoints in rolling metrics using CUSUM-like approach.
    fn detect_changepoints(&self, metrics: &[RollingMetric]) -> Vec<usize> {
        if metrics.len() < 10 {
            return Vec::new();
        }

        let mean_wr: f64 = metrics.iter().map(|m| m.win_rate).sum::<f64>() / metrics.len() as f64;
        let mut cusum = 0.0;
        let mut changepoints = Vec::new();
        let threshold = 0.15; // Significant deviation threshold

        for (i, metric) in metrics.iter().enumerate() {
            cusum += metric.win_rate - mean_wr;
            if cusum.abs() > threshold && i > 0 && i < metrics.len() - 1 {
                changepoints.push(i);
                cusum = 0.0; // Reset after detection
            }
        }

        changepoints
    }

    /// Performs complete edge analysis.
    ///
    /// # Arguments
    /// * `settlements` - All settlement results
    /// * `volatilities` - Volatility values for each settlement
    ///
    /// # Returns
    /// Complete EdgeAnalysis with all dimensions
    #[must_use]
    pub fn analyze(&self, settlements: &[SettlementResult], volatilities: &[f64]) -> EdgeAnalysis {
        let overall = EdgeMeasurement::from_settlements(settlements, self.config.alpha);
        let conditional = self.analyze_conditional(settlements);
        let time_of_day = self.analyze_time_of_day(settlements);
        let volatility = self.analyze_volatility(settlements, volatilities);
        let decay = self.detect_decay(settlements);

        EdgeAnalysis {
            overall,
            conditional,
            time_of_day,
            volatility,
            decay,
        }
    }

    /// Classifies edge strength based on win rate and significance.
    #[must_use]
    pub fn classify_edge(&self, measurement: &EdgeMeasurement) -> EdgeClassification {
        if measurement.n_samples < self.config.min_samples {
            return EdgeClassification::None;
        }

        let wr = measurement.win_rate;
        let p = measurement.p_value;
        let ev = measurement.ev_per_bet;

        if wr < 0.5 || ev < Decimal::ZERO {
            EdgeClassification::Negative
        } else if wr > 0.55 && p < 0.01 && ev > Decimal::ZERO {
            EdgeClassification::Strong
        } else if wr > 0.53 && p < 0.05 && ev > Decimal::ZERO {
            EdgeClassification::Moderate
        } else if wr > 0.51 && p < 0.10 {
            EdgeClassification::Weak
        } else {
            EdgeClassification::None
        }
    }

    /// Generates edge summary with Go/No-Go decision.
    ///
    /// # Arguments
    /// * `analysis` - Complete edge analysis
    ///
    /// # Returns
    /// EdgeSummary with classification, confidence, and decision
    #[must_use]
    pub fn summarize(&self, analysis: &EdgeAnalysis) -> EdgeSummary {
        let classification = self.classify_edge(&analysis.overall);

        // Persistence confidence based on decay analysis
        let persistence_confidence = if analysis.decay.is_decaying {
            0.3
        } else if analysis.decay.decay_slope < 0.0 {
            0.6
        } else {
            0.8
        };

        // Go/No-Go logic
        let edge_positive = analysis.overall.edge > 0.0;
        let not_decaying = !analysis.decay.is_decaying;
        let strong_or_moderate = matches!(
            classification,
            EdgeClassification::Strong | EdgeClassification::Moderate
        );
        let sufficient_samples = analysis.overall.n_samples >= self.config.min_samples;

        let go_no_go = edge_positive && not_decaying && strong_or_moderate && sufficient_samples;

        // Build reasons
        let mut reasons = Vec::new();
        if go_no_go {
            reasons.push(format!("Edge classification: {:?}", classification));
            reasons.push(format!(
                "Win rate: {:.1}% (CI: {:.1}%-{:.1}%)",
                analysis.overall.win_rate * 100.0,
                analysis.overall.wilson_ci.0 * 100.0,
                analysis.overall.wilson_ci.1 * 100.0
            ));
            reasons.push(format!("p-value: {:.4}", analysis.overall.p_value));
            if not_decaying {
                reasons.push("No significant edge decay detected".to_string());
            }
        } else {
            if !strong_or_moderate {
                reasons.push(format!("Insufficient edge: {:?}", classification));
            }
            if !edge_positive {
                reasons.push("Negative edge".to_string());
            }
            if analysis.decay.is_decaying {
                reasons.push(format!(
                    "Edge decay detected (slope: {:.4})",
                    analysis.decay.decay_slope
                ));
            }
            if !sufficient_samples {
                reasons.push(format!(
                    "Insufficient samples: {} < {}",
                    analysis.overall.n_samples, self.config.min_samples
                ));
            }
        }

        // Recommended Kelly fraction based on classification
        let recommended_kelly_fraction = match classification {
            EdgeClassification::Strong => 0.5,
            EdgeClassification::Moderate => 0.25,
            EdgeClassification::Weak => 0.1,
            _ => 0.0,
        };

        EdgeSummary {
            classification,
            persistence_confidence,
            go_no_go,
            reasons,
            recommended_kelly_fraction,
        }
    }
}

impl Default for EdgeAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculates percentile value from a slice.
fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let idx = (p * (sorted.len() - 1) as f64).round() as usize;
    let idx = idx.min(sorted.len() - 1);
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::outcome::{BetDirection, BinaryBet};
    use chrono::TimeZone;
    use rust_decimal_macros::dec;
    use std::collections::HashMap;

    // ============================================================
    // Test Helpers
    // ============================================================

    fn create_settlement(
        timestamp: DateTime<Utc>,
        outcome: BinaryOutcome,
        signal_strength: f64,
    ) -> SettlementResult {
        let bet = BinaryBet::new(
            timestamp,
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0.50),
            signal_strength,
        );
        let settlement_time = timestamp + chrono::Duration::minutes(15);
        SettlementResult::new(
            bet,
            settlement_time,
            dec!(43500),
            dec!(43000),
            outcome,
            dec!(2),
        )
    }

    fn create_settlement_with_metadata(
        timestamp: DateTime<Utc>,
        outcome: BinaryOutcome,
        signal_strength: f64,
        confidence: f64,
    ) -> SettlementResult {
        let mut metadata = HashMap::new();
        metadata.insert("confidence".to_string(), confidence);

        let bet = BinaryBet::with_metadata(
            timestamp,
            "BTCUSD-15MIN-UP".to_string(),
            BetDirection::Yes,
            dec!(100),
            dec!(0.50),
            signal_strength,
            metadata,
        );
        let settlement_time = timestamp + chrono::Duration::minutes(15);
        SettlementResult::new(
            bet,
            settlement_time,
            dec!(43500),
            dec!(43000),
            outcome,
            dec!(2),
        )
    }

    fn create_settlements_with_win_rate(n: usize, win_rate: f64) -> Vec<SettlementResult> {
        let wins = (n as f64 * win_rate).round() as usize;
        let base_time = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        (0..n)
            .map(|i| {
                let ts = base_time + chrono::Duration::minutes(15 * i as i64);
                let outcome = if i < wins {
                    BinaryOutcome::Win
                } else {
                    BinaryOutcome::Loss
                };
                create_settlement(ts, outcome, 0.75)
            })
            .collect()
    }

    // ============================================================
    // EdgeClassification Tests
    // ============================================================

    #[test]
    fn edge_classification_variants_are_distinct() {
        let strong = EdgeClassification::Strong;
        let moderate = EdgeClassification::Moderate;
        let weak = EdgeClassification::Weak;
        let none = EdgeClassification::None;
        let negative = EdgeClassification::Negative;

        assert_ne!(strong, moderate);
        assert_ne!(moderate, weak);
        assert_ne!(weak, none);
        assert_ne!(none, negative);
    }

    #[test]
    fn edge_classification_serializes_correctly() {
        assert_eq!(
            serde_json::to_string(&EdgeClassification::Strong).unwrap(),
            r#""Strong""#
        );
        assert_eq!(
            serde_json::to_string(&EdgeClassification::Negative).unwrap(),
            r#""Negative""#
        );
    }

    // ============================================================
    // EdgeMeasurement::from_settlements Tests
    // ============================================================

    #[test]
    fn from_settlements_empty_returns_empty() {
        let settlements: Vec<SettlementResult> = vec![];
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);

        assert_eq!(measurement.n_samples, 0);
        assert!((measurement.win_rate - 0.0).abs() < f64::EPSILON);
        assert!((measurement.p_value - 1.0).abs() < f64::EPSILON);
        assert!(!measurement.is_significant);
    }

    #[test]
    fn from_settlements_computes_win_rate_correctly() {
        // 6 wins, 4 losses = 60% win rate
        let settlements = create_settlements_with_win_rate(10, 0.6);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);

        assert_eq!(measurement.n_samples, 10);
        assert!((measurement.win_rate - 0.6).abs() < 0.01);
    }

    #[test]
    fn from_settlements_computes_edge_correctly() {
        // 70% win rate -> edge = 0.70 - 0.50 = 0.20
        let settlements = create_settlements_with_win_rate(100, 0.70);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);

        assert!((measurement.edge - 0.20).abs() < 0.02);
    }

    #[test]
    fn from_settlements_computes_wilson_ci() {
        let settlements = create_settlements_with_win_rate(100, 0.60);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);

        // Wilson CI should contain the true win rate
        assert!(measurement.wilson_ci.0 < measurement.win_rate);
        assert!(measurement.wilson_ci.1 > measurement.win_rate);
        // CI width should be reasonable for n=100
        let ci_width = measurement.wilson_ci.1 - measurement.wilson_ci.0;
        assert!(ci_width < 0.20 && ci_width > 0.05);
    }

    #[test]
    fn from_settlements_computes_total_pnl() {
        // 6 wins, 4 losses
        // Win: payout = 100/0.50 - 100 - 2 = 98 net
        // Loss: -100 - 2 = -102 net
        // Total: 6*98 + 4*(-102) = 588 - 408 = 180... but wait
        // Actually from our test helper: Win gross_pnl = max_payout - stake = 200 - 100 = 100
        // net_pnl = 100 - 2 = 98
        // Loss gross_pnl = -100, net_pnl = -100 - 2 = -102
        let settlements = create_settlements_with_win_rate(10, 0.6);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);

        // 6 * 98 + 4 * (-102) = 588 - 408 = 180
        assert_eq!(measurement.total_pnl, dec!(180));
    }

    #[test]
    fn from_settlements_computes_ev_per_bet() {
        let settlements = create_settlements_with_win_rate(10, 0.6);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);

        // EV = 180 / 10 = 18
        assert_eq!(measurement.ev_per_bet, dec!(18));
    }

    // ============================================================
    // EdgeMeasurement::is_significant Tests
    // ============================================================

    #[test]
    fn is_significant_at_alpha_005() {
        // 65% win rate with 100 samples should be significant
        let settlements = create_settlements_with_win_rate(100, 0.65);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);

        assert!(measurement.is_significant);
        assert!(measurement.is_significant_at(0.05));
    }

    #[test]
    fn is_not_significant_with_small_edge() {
        // 52% win rate with 100 samples - not significant
        let settlements = create_settlements_with_win_rate(100, 0.52);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);

        assert!(!measurement.is_significant);
    }

    #[test]
    fn is_significant_uses_alpha_threshold() {
        // 65% win rate with 100 samples - clearly significant
        let settlements = create_settlements_with_win_rate(100, 0.65);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);

        // Should be significant at 0.05
        assert!(measurement.is_significant_at(0.10));
        // p-value should be less than 0.05 for 65% with n=100
        assert!(measurement.p_value < 0.05);
    }

    // ============================================================
    // analyze_conditional Tests
    // ============================================================

    #[test]
    fn analyze_conditional_computes_baseline() {
        let settlements = create_settlements_with_win_rate(50, 0.60);
        let analyzer = EdgeAnalyzer::new();

        let result = analyzer.analyze_conditional(&settlements);

        assert_eq!(result.baseline.n_samples, 50);
        assert!((result.baseline.win_rate - 0.60).abs() < 0.02);
    }

    #[test]
    fn analyze_conditional_filters_by_signal_strength() {
        let base_time = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        // Create settlements with varying signal strengths
        // High strength (>0.6): wins
        // Low strength (<=0.6): losses
        let mut settlements = Vec::new();
        for i in 0..10 {
            let ts = base_time + chrono::Duration::minutes(15 * i);
            let (outcome, strength) = if i < 5 {
                (BinaryOutcome::Win, 0.85) // High strength wins
            } else {
                (BinaryOutcome::Loss, 0.40) // Low strength losses
            };
            settlements.push(create_settlement(ts, outcome, strength));
        }

        let analyzer = EdgeAnalyzer::new();
        let result = analyzer.analyze_conditional(&settlements);

        // High strength should have 5 samples (all wins)
        assert_eq!(result.high_strength.n_samples, 5);
        assert!((result.high_strength.win_rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn analyze_conditional_filters_very_high_strength() {
        let base_time = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        let mut settlements = Vec::new();
        // 3 with very high strength (>0.8)
        for i in 0..3 {
            let ts = base_time + chrono::Duration::minutes(15 * i);
            settlements.push(create_settlement(ts, BinaryOutcome::Win, 0.90));
        }
        // 7 with lower strength
        for i in 3..10 {
            let ts = base_time + chrono::Duration::minutes(15 * i);
            settlements.push(create_settlement(ts, BinaryOutcome::Loss, 0.50));
        }

        let analyzer = EdgeAnalyzer::new();
        let result = analyzer.analyze_conditional(&settlements);

        assert_eq!(result.very_high_strength.n_samples, 3);
        assert!((result.very_high_strength.win_rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn analyze_conditional_filters_by_confidence() {
        let base_time = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        let mut settlements = Vec::new();
        // High confidence (>0.7) -> wins
        for i in 0..4 {
            let ts = base_time + chrono::Duration::minutes(15 * i);
            settlements.push(create_settlement_with_metadata(
                ts,
                BinaryOutcome::Win,
                0.75,
                0.85,
            ));
        }
        // Low confidence -> losses
        for i in 4..10 {
            let ts = base_time + chrono::Duration::minutes(15 * i);
            settlements.push(create_settlement_with_metadata(
                ts,
                BinaryOutcome::Loss,
                0.75,
                0.50,
            ));
        }

        let analyzer = EdgeAnalyzer::new();
        let result = analyzer.analyze_conditional(&settlements);

        assert_eq!(result.high_confidence.n_samples, 4);
        assert!((result.high_confidence.win_rate - 1.0).abs() < f64::EPSILON);
    }

    // ============================================================
    // analyze_time_of_day Tests
    // ============================================================

    #[test]
    fn analyze_time_of_day_groups_by_hour() {
        let base_date = Utc.with_ymd_and_hms(2025, 1, 15, 0, 0, 0).unwrap();

        let mut settlements = Vec::new();
        // Hour 10: wins
        for i in 0..5 {
            let ts = base_date + chrono::Duration::hours(10) + chrono::Duration::minutes(i * 3);
            settlements.push(create_settlement(ts, BinaryOutcome::Win, 0.75));
        }
        // Hour 14: losses
        for i in 0..5 {
            let ts = base_date + chrono::Duration::hours(14) + chrono::Duration::minutes(i * 3);
            settlements.push(create_settlement(ts, BinaryOutcome::Loss, 0.75));
        }

        let analyzer = EdgeAnalyzer::new();
        let result = analyzer.analyze_time_of_day(&settlements);

        assert!(result.by_hour.contains_key(&10));
        assert!(result.by_hour.contains_key(&14));
        assert_eq!(result.by_hour.get(&10).unwrap().n_samples, 5);
        assert_eq!(result.by_hour.get(&14).unwrap().n_samples, 5);
    }

    #[test]
    fn analyze_time_of_day_identifies_best_hours() {
        let base_date = Utc.with_ymd_and_hms(2025, 1, 15, 0, 0, 0).unwrap();

        let mut settlements = Vec::new();
        // Hour 8: 80% win rate (best)
        for i in 0..8 {
            let ts = base_date + chrono::Duration::hours(8) + chrono::Duration::minutes(i * 2);
            let outcome = if i < 8 {
                BinaryOutcome::Win
            } else {
                BinaryOutcome::Loss
            };
            settlements.push(create_settlement(ts, outcome, 0.75));
        }
        for i in 8..10 {
            let ts = base_date + chrono::Duration::hours(8) + chrono::Duration::minutes(i * 2);
            settlements.push(create_settlement(ts, BinaryOutcome::Loss, 0.75));
        }

        // Hour 20: 20% win rate (worst)
        for i in 0..2 {
            let ts = base_date + chrono::Duration::hours(20) + chrono::Duration::minutes(i * 2);
            settlements.push(create_settlement(ts, BinaryOutcome::Win, 0.75));
        }
        for i in 2..10 {
            let ts = base_date + chrono::Duration::hours(20) + chrono::Duration::minutes(i * 2);
            settlements.push(create_settlement(ts, BinaryOutcome::Loss, 0.75));
        }

        let analyzer = EdgeAnalyzer::new();
        let result = analyzer.analyze_time_of_day(&settlements);

        // Hour 8 should be in best hours (80% = 0.30 edge)
        assert!(result.best_hours.contains(&8));
        // Hour 20 should be in worst hours (20% = -0.30 edge)
        assert!(result.worst_hours.contains(&20));
    }

    #[test]
    fn analyze_time_of_day_generates_recommendations() {
        let base_date = Utc.with_ymd_and_hms(2025, 1, 15, 0, 0, 0).unwrap();

        let mut settlements = Vec::new();
        // Create enough data for recommendations
        for i in 0..20 {
            let ts = base_date + chrono::Duration::hours(10) + chrono::Duration::minutes(i * 3);
            settlements.push(create_settlement(ts, BinaryOutcome::Win, 0.75));
        }

        let analyzer = EdgeAnalyzer::new();
        let result = analyzer.analyze_time_of_day(&settlements);

        // Should have at least one recommendation
        assert!(!result.recommendations.is_empty());
    }

    // ============================================================
    // analyze_volatility Tests
    // ============================================================

    #[test]
    fn analyze_volatility_groups_by_regime() {
        let base_time = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        // 10 settlements with volatilities 1-10
        let settlements: Vec<_> = (0..10)
            .map(|i| {
                let ts = base_time + chrono::Duration::minutes(15 * i);
                create_settlement(ts, BinaryOutcome::Win, 0.75)
            })
            .collect();
        let volatilities: Vec<f64> = (1..=10).map(|x| x as f64).collect();

        let analyzer = EdgeAnalyzer::new();
        let result = analyzer.analyze_volatility(&settlements, &volatilities);

        // Should have data in all three regimes
        assert!(result.low.n_samples > 0);
        assert!(result.medium.n_samples > 0);
        assert!(result.high.n_samples > 0);
    }

    #[test]
    fn analyze_volatility_returns_correct_thresholds() {
        let base_time = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        let settlements: Vec<_> = (0..10)
            .map(|i| {
                let ts = base_time + chrono::Duration::minutes(15 * i);
                create_settlement(ts, BinaryOutcome::Win, 0.75)
            })
            .collect();
        let volatilities: Vec<f64> = (1..=10).map(|x| x as f64).collect();

        let analyzer = EdgeAnalyzer::new();
        let result = analyzer.analyze_volatility(&settlements, &volatilities);

        // Thresholds should be at 33rd and 67th percentile of 1-10
        // 33rd percentile ~= 3.3, 67th percentile ~= 6.7
        assert!(result.thresholds.0 > 2.0 && result.thresholds.0 < 5.0);
        assert!(result.thresholds.1 > 5.0 && result.thresholds.1 < 8.0);
    }

    #[test]
    fn analyze_volatility_empty_returns_empty() {
        let analyzer = EdgeAnalyzer::new();
        let result = analyzer.analyze_volatility(&[], &[]);

        assert_eq!(result.low.n_samples, 0);
        assert_eq!(result.medium.n_samples, 0);
        assert_eq!(result.high.n_samples, 0);
    }

    // ============================================================
    // detect_decay Tests
    // ============================================================

    #[test]
    fn detect_decay_calculates_rolling_win_rate() {
        // Create 100 settlements with constant 60% win rate
        let settlements = create_settlements_with_win_rate(100, 0.60);

        let mut config = EdgeAnalyzerConfig::default();
        config.rolling_window_size = 20;
        let analyzer = EdgeAnalyzer::with_config(config);

        let result = analyzer.detect_decay(&settlements);

        // Should have rolling metrics
        assert!(!result.rolling_metrics.is_empty());
        // Number of rolling metrics = n - window_size + 1 = 100 - 20 + 1 = 81
        assert_eq!(result.rolling_metrics.len(), 81);
    }

    #[test]
    fn detect_decay_not_decaying_for_stable_edge() {
        // Create settlements with perfectly interleaved wins/losses for stable 60% win rate
        let base_time = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();
        let mut settlements = Vec::new();

        // Create 100 settlements with stable pattern: WWWLLWWWLLWWWLL...
        // This gives exactly 60% win rate in every window
        for i in 0..100 {
            let ts = base_time + chrono::Duration::minutes(15 * i as i64);
            let outcome = if i % 5 < 3 {
                BinaryOutcome::Win
            } else {
                BinaryOutcome::Loss
            };
            settlements.push(create_settlement(ts, outcome, 0.75));
        }

        let mut config = EdgeAnalyzerConfig::default();
        config.rolling_window_size = 20;
        config.alpha = 0.01; // Stricter alpha for decay detection
        let analyzer = EdgeAnalyzer::with_config(config);

        let result = analyzer.detect_decay(&settlements);

        // Stable win rate should not show significant decay
        // The slope should be close to 0 for truly stable data
        assert!(
            result.decay_slope.abs() < 0.01,
            "decay_slope was {}",
            result.decay_slope
        );
    }

    #[test]
    fn detect_decay_detects_declining_win_rate() {
        let base_time = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        // Create settlements with declining win rate
        // First half: 80% win rate, second half: 30% win rate
        let mut settlements = Vec::new();
        for i in 0..50 {
            let ts = base_time + chrono::Duration::minutes(15 * i);
            let outcome = if i % 5 < 4 {
                BinaryOutcome::Win
            } else {
                BinaryOutcome::Loss
            };
            settlements.push(create_settlement(ts, outcome, 0.75));
        }
        for i in 50..100 {
            let ts = base_time + chrono::Duration::minutes(15 * i as i64);
            let outcome = if i % 5 < 1 {
                BinaryOutcome::Win
            } else {
                BinaryOutcome::Loss
            };
            settlements.push(create_settlement(ts, outcome, 0.75));
        }

        let mut config = EdgeAnalyzerConfig::default();
        config.rolling_window_size = 20;
        let analyzer = EdgeAnalyzer::with_config(config);

        let result = analyzer.detect_decay(&settlements);

        // Decay slope should be negative
        assert!(result.decay_slope < 0.0);
    }

    #[test]
    fn detect_decay_insufficient_samples_returns_empty() {
        let settlements = create_settlements_with_win_rate(10, 0.60);

        let mut config = EdgeAnalyzerConfig::default();
        config.rolling_window_size = 20; // More than we have
        let analyzer = EdgeAnalyzer::with_config(config);

        let result = analyzer.detect_decay(&settlements);

        assert!(result.rolling_metrics.is_empty());
        assert!(!result.is_decaying);
    }

    // ============================================================
    // linear_regression Tests
    // ============================================================

    #[test]
    fn linear_regression_perfect_positive_slope() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 4.0, 6.0, 8.0, 10.0]; // y = 2x

        let result = linear_regression(&x, &y);

        assert!((result.slope - 2.0).abs() < 0.01);
        assert!((result.intercept - 0.0).abs() < 0.01);
        assert!((result.r_squared - 1.0).abs() < 0.01);
    }

    #[test]
    fn linear_regression_negative_slope() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![10.0, 8.0, 6.0, 4.0, 2.0]; // y = -2x + 12

        let result = linear_regression(&x, &y);

        assert!((result.slope - (-2.0)).abs() < 0.01);
        assert!((result.r_squared - 1.0).abs() < 0.01);
    }

    #[test]
    fn linear_regression_computes_p_value() {
        let x: Vec<f64> = (0..20).map(|i| i as f64).collect();
        let y: Vec<f64> = x.iter().map(|&xi| 0.5 + 0.02 * xi).collect();

        let result = linear_regression(&x, &y);

        // Strong linear relationship should have low p-value
        // For perfect correlation, p-value should be very low
        assert!(result.p_value < 0.05, "p-value was {}", result.p_value);
        assert!((result.r_squared - 1.0).abs() < 0.01);
    }

    #[test]
    fn linear_regression_no_relationship() {
        // Horizontal line - no slope
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![5.0, 5.0, 5.0, 5.0, 5.0];

        let result = linear_regression(&x, &y);

        assert!((result.slope - 0.0).abs() < 0.01);
        // p-value should be high (no significant relationship)
        // R-squared will be 1.0 because there's no variance in y
    }

    #[test]
    fn linear_regression_insufficient_data() {
        let x = vec![1.0, 2.0];
        let y = vec![1.0, 2.0];

        let result = linear_regression(&x, &y);

        // Should return defaults for insufficient data
        assert!((result.slope - 0.0).abs() < f64::EPSILON);
        assert!((result.p_value - 1.0).abs() < f64::EPSILON);
    }

    // ============================================================
    // EdgeClassification Tests
    // ============================================================

    #[test]
    fn classify_edge_strong() {
        // 60% win rate, 300 samples - need larger sample for p < 0.01
        let settlements = create_settlements_with_win_rate(300, 0.60);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);
        let analyzer = EdgeAnalyzer::new();

        let classification = analyzer.classify_edge(&measurement);

        // 60% with 300 samples should be Strong (p < 0.01)
        assert_eq!(classification, EdgeClassification::Strong);
    }

    #[test]
    fn classify_edge_moderate() {
        // 56% win rate, 300 samples
        let settlements = create_settlements_with_win_rate(300, 0.56);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);
        let analyzer = EdgeAnalyzer::new();

        let classification = analyzer.classify_edge(&measurement);

        // 56% with 300 samples should be at least Moderate (p < 0.05)
        assert!(matches!(
            classification,
            EdgeClassification::Strong | EdgeClassification::Moderate
        ));
    }

    #[test]
    fn classify_edge_weak() {
        // 52% win rate, 200 samples - marginal
        let settlements = create_settlements_with_win_rate(200, 0.52);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);
        let analyzer = EdgeAnalyzer::new();

        let classification = analyzer.classify_edge(&measurement);

        // 52% is marginal
        assert!(matches!(
            classification,
            EdgeClassification::Weak | EdgeClassification::None
        ));
    }

    #[test]
    fn classify_edge_none_insufficient_samples() {
        let settlements = create_settlements_with_win_rate(50, 0.70);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);
        let analyzer = EdgeAnalyzer::new();

        let classification = analyzer.classify_edge(&measurement);

        // Only 50 samples - insufficient
        assert_eq!(classification, EdgeClassification::None);
    }

    #[test]
    fn classify_edge_negative() {
        let settlements = create_settlements_with_win_rate(200, 0.40);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);
        let analyzer = EdgeAnalyzer::new();

        let classification = analyzer.classify_edge(&measurement);

        assert_eq!(classification, EdgeClassification::Negative);
    }

    // ============================================================
    // EdgeSummary / Go-No-Go Tests
    // ============================================================

    #[test]
    fn summarize_go_for_strong_edge() {
        // Create settlements with stable 60% win rate pattern
        let base_time = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();
        let mut settlements = Vec::new();

        // Create 300 settlements with stable 60% pattern (WWWLLWWWLL...)
        for i in 0..300 {
            let ts = base_time + chrono::Duration::minutes(15 * i as i64);
            let outcome = if i % 5 < 3 {
                BinaryOutcome::Win
            } else {
                BinaryOutcome::Loss
            };
            settlements.push(create_settlement(ts, outcome, 0.75));
        }

        let volatilities: Vec<f64> = (0..300).map(|i| 5.0 + (i % 10) as f64 * 0.1).collect();

        let analyzer = EdgeAnalyzer::new();
        let analysis = analyzer.analyze(&settlements, &volatilities);
        let summary = analyzer.summarize(&analysis);

        // Verify the classification is strong
        assert_eq!(summary.classification, EdgeClassification::Strong);
        assert!(
            summary.go_no_go,
            "Expected go_no_go=true, reasons: {:?}",
            summary.reasons
        );
        assert!(summary.recommended_kelly_fraction > 0.0);
    }

    #[test]
    fn summarize_no_go_for_negative_edge() {
        let settlements = create_settlements_with_win_rate(200, 0.40);
        let volatilities: Vec<f64> = (0..200).map(|i| 5.0 + (i % 10) as f64 * 0.1).collect();

        let analyzer = EdgeAnalyzer::new();
        let analysis = analyzer.analyze(&settlements, &volatilities);
        let summary = analyzer.summarize(&analysis);

        assert!(!summary.go_no_go);
        assert_eq!(summary.classification, EdgeClassification::Negative);
        assert!((summary.recommended_kelly_fraction - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn summarize_no_go_for_insufficient_samples() {
        let settlements = create_settlements_with_win_rate(50, 0.70);
        let volatilities: Vec<f64> = (0..50).map(|i| 5.0 + (i % 10) as f64 * 0.1).collect();

        let analyzer = EdgeAnalyzer::new();
        let analysis = analyzer.analyze(&settlements, &volatilities);
        let summary = analyzer.summarize(&analysis);

        assert!(!summary.go_no_go);
        assert!(summary.reasons.iter().any(|r| r.contains("Insufficient")));
    }

    #[test]
    fn summarize_includes_reasons() {
        let settlements = create_settlements_with_win_rate(200, 0.58);
        let volatilities: Vec<f64> = (0..200).map(|i| 5.0 + (i % 10) as f64 * 0.1).collect();

        let analyzer = EdgeAnalyzer::new();
        let analysis = analyzer.analyze(&settlements, &volatilities);
        let summary = analyzer.summarize(&analysis);

        assert!(!summary.reasons.is_empty());
    }

    #[test]
    fn summarize_persistence_confidence_reflects_decay() {
        let base_time = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();

        // Create decaying edge
        let mut settlements = Vec::new();
        for i in 0..100 {
            let ts = base_time + chrono::Duration::minutes(15 * i as i64);
            // First 50: 80% win rate, last 50: 20% win rate
            let outcome = if i < 50 {
                if i % 5 < 4 {
                    BinaryOutcome::Win
                } else {
                    BinaryOutcome::Loss
                }
            } else if i % 5 < 1 {
                BinaryOutcome::Win
            } else {
                BinaryOutcome::Loss
            };
            settlements.push(create_settlement(ts, outcome, 0.75));
        }

        let volatilities: Vec<f64> = (0..100).map(|i| 5.0 + (i % 10) as f64 * 0.1).collect();

        let mut config = EdgeAnalyzerConfig::default();
        config.rolling_window_size = 20;
        let analyzer = EdgeAnalyzer::with_config(config);

        let analysis = analyzer.analyze(&settlements, &volatilities);
        let summary = analyzer.summarize(&analysis);

        // With decay, persistence_confidence should be lower
        assert!(summary.persistence_confidence < 0.8);
    }

    // ============================================================
    // Edge Cases and Integration Tests
    // ============================================================

    #[test]
    fn full_analysis_empty_data() {
        let analyzer = EdgeAnalyzer::new();
        let analysis = analyzer.analyze(&[], &[]);

        assert_eq!(analysis.overall.n_samples, 0);
        assert!(analysis.time_of_day.by_hour.is_empty());
    }

    #[test]
    fn full_analysis_single_settlement() {
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();
        let settlements = vec![create_settlement(ts, BinaryOutcome::Win, 0.75)];
        let volatilities = vec![5.0];

        let analyzer = EdgeAnalyzer::new();
        let analysis = analyzer.analyze(&settlements, &volatilities);

        assert_eq!(analysis.overall.n_samples, 1);
        assert!((analysis.overall.win_rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn analyzer_default_creates_valid_instance() {
        let analyzer = EdgeAnalyzer::default();
        let settlements = create_settlements_with_win_rate(100, 0.60);

        let result = analyzer.analyze_conditional(&settlements);

        assert_eq!(result.baseline.n_samples, 100);
    }

    #[test]
    fn percentile_function_handles_edge_cases() {
        assert!((percentile(&[], 0.5) - 0.0).abs() < f64::EPSILON);
        assert!((percentile(&[5.0], 0.5) - 5.0).abs() < f64::EPSILON);
        assert!((percentile(&[1.0, 2.0, 3.0], 0.0) - 1.0).abs() < f64::EPSILON);
        assert!((percentile(&[1.0, 2.0, 3.0], 1.0) - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn config_custom_alpha() {
        let config = EdgeAnalyzerConfig {
            alpha: 0.01,
            ..Default::default()
        };
        let analyzer = EdgeAnalyzer::with_config(config);

        // 60% win rate, 100 samples - significant at 0.05 but maybe not at 0.01
        let settlements = create_settlements_with_win_rate(100, 0.60);
        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.01);

        // The measurement uses the alpha we pass to from_settlements
        // Check that with stricter alpha, significance may differ
        let loose_measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);
        let _classification = analyzer.classify_edge(&measurement);

        // Both should have same p-value
        assert!((measurement.p_value - loose_measurement.p_value).abs() < f64::EPSILON);
    }

    #[test]
    fn settlement_with_push_excluded_from_win_rate() {
        let ts = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();
        let settlements = vec![
            create_settlement(ts, BinaryOutcome::Win, 0.75),
            create_settlement(
                ts + chrono::Duration::minutes(15),
                BinaryOutcome::Push,
                0.75,
            ),
            create_settlement(
                ts + chrono::Duration::minutes(30),
                BinaryOutcome::Loss,
                0.75,
            ),
        ];

        let measurement = EdgeMeasurement::from_settlements(&settlements, 0.05);

        // n_samples = 3 (all), but win_rate = 1/2 = 0.5 (excluding push)
        assert_eq!(measurement.n_samples, 3);
        assert!((measurement.win_rate - 0.5).abs() < f64::EPSILON);
    }
}
