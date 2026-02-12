//! Composite signal generator.
//!
//! Combines multiple signal generators into a unified signal using
//! various aggregation methods including weighted averaging, voting,
//! Bayesian combination, and multicollinearity-aware weighting.

use algo_trade_core::{Direction, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;

/// Method for combining multiple signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombinationMethod {
    /// Weighted average of signal directions and strengths
    WeightedAverage,
    /// Majority vote on direction
    Voting,
    /// Use the strongest signal
    Strongest,
    /// Bayesian combination using log-odds
    Bayesian,
    /// Require at least N signals to agree on direction
    RequireN {
        /// Minimum number of signals that must agree
        min_agree: usize,
    },
}

/// Correlation matrix for tracking signal correlations.
///
/// Used for multicollinearity detection to adjust signal weights
/// when signals are highly correlated.
#[derive(Debug, Clone)]
pub struct CorrelationMatrix {
    /// Names of signals in the matrix
    pub signal_names: Vec<String>,
    /// Correlation values (symmetric matrix)
    pub matrix: Vec<Vec<f64>>,
}

impl CorrelationMatrix {
    /// Creates a new correlation matrix with the given signal names.
    ///
    /// Initializes all correlations to 0.0 except diagonal (1.0).
    #[must_use]
    pub fn new(signal_names: Vec<String>) -> Self {
        let n = signal_names.len();
        let mut matrix = vec![vec![0.0; n]; n];
        // Diagonal is always 1.0 (self-correlation)
        for (i, row) in matrix.iter_mut().enumerate() {
            row[i] = 1.0;
        }
        Self {
            signal_names,
            matrix,
        }
    }

    /// Sets the correlation between signals at indices i and j.
    ///
    /// Also sets the symmetric entry (j, i).
    pub fn set(&mut self, i: usize, j: usize, correlation: f64) {
        if i < self.matrix.len() && j < self.matrix.len() {
            let clamped = correlation.clamp(-1.0, 1.0);
            self.matrix[i][j] = clamped;
            self.matrix[j][i] = clamped;
        }
    }

    /// Gets the correlation between signals at indices i and j.
    #[must_use]
    pub fn get(&self, i: usize, j: usize) -> f64 {
        if i < self.matrix.len() && j < self.matrix.len() {
            self.matrix[i][j]
        } else {
            0.0
        }
    }

    /// Gets the correlation between signals by name.
    #[must_use]
    pub fn get_by_name(&self, name1: &str, name2: &str) -> Option<f64> {
        let i = self.signal_names.iter().position(|n| n == name1)?;
        let j = self.signal_names.iter().position(|n| n == name2)?;
        Some(self.get(i, j))
    }

    /// Returns the number of signals in the matrix.
    #[must_use]
    pub fn size(&self) -> usize {
        self.signal_names.len()
    }
}

/// Calculates the Pearson correlation coefficient between two series.
fn pearson_correlation(x: &[f64], y: &[f64]) -> f64 {
    if x.len() != y.len() || x.len() < 2 {
        return 0.0;
    }

    let n = x.len() as f64;
    let mean_x = x.iter().sum::<f64>() / n;
    let mean_y = y.iter().sum::<f64>() / n;

    let mut covariance = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;

    for (xi, yi) in x.iter().zip(y.iter()) {
        let dx = xi - mean_x;
        let dy = yi - mean_y;
        covariance += dx * dy;
        var_x += dx * dx;
        var_y += dy * dy;
    }

    let denominator = (var_x * var_y).sqrt();
    if denominator < f64::EPSILON {
        return 0.0;
    }

    covariance / denominator
}

/// Calculates a correlation matrix from historical signal values.
///
/// # Arguments
/// * `historical_signals` - Vector of signal snapshots, each containing signal_name -> SignalValue
/// * `signal_names` - Names of signals to include in the matrix
///
/// # Returns
/// A correlation matrix for the specified signals
pub fn calculate_correlation_matrix(
    historical_signals: &[HashMap<String, SignalValue>],
    signal_names: &[String],
) -> CorrelationMatrix {
    let mut matrix = CorrelationMatrix::new(signal_names.to_vec());

    if historical_signals.is_empty() || signal_names.is_empty() {
        return matrix;
    }

    // Extract signal series for each signal name
    let series: Vec<Vec<f64>> = signal_names
        .iter()
        .map(|name| {
            historical_signals
                .iter()
                .filter_map(|snapshot| {
                    snapshot.get(name).map(|sv| {
                        let sign = match sv.direction {
                            Direction::Up => 1.0,
                            Direction::Down => -1.0,
                            Direction::Neutral => 0.0,
                        };
                        sign * sv.strength
                    })
                })
                .collect()
        })
        .collect();

    // Calculate pairwise correlations
    for i in 0..signal_names.len() {
        for j in (i + 1)..signal_names.len() {
            let correlation = pearson_correlation(&series[i], &series[j]);
            matrix.set(i, j, correlation);
        }
    }

    matrix
}

/// Adjusts weights based on multicollinearity.
///
/// For signals with correlation above the threshold, weights are reduced
/// proportionally to the number of highly correlated pairs.
///
/// # Arguments
/// * `weights` - Mutable map of signal_name -> weight
/// * `matrix` - Correlation matrix
/// * `threshold` - Correlation threshold (e.g., 0.7) above which to adjust
pub fn adjust_weights_for_multicollinearity(
    weights: &mut HashMap<String, f64>,
    matrix: &CorrelationMatrix,
    threshold: f64,
) {
    // Count high correlations for each signal
    let mut high_correlation_counts: HashMap<String, usize> = HashMap::new();

    for (i, name_i) in matrix.signal_names.iter().enumerate() {
        let mut count = 0;
        for (j, _name_j) in matrix.signal_names.iter().enumerate() {
            if i != j && matrix.get(i, j).abs() > threshold {
                count += 1;
            }
        }
        high_correlation_counts.insert(name_i.clone(), count);
    }

    // Adjust weights: new_weight = old_weight / (1 + count)
    for (name, count) in high_correlation_counts {
        if count > 0 {
            if let Some(weight) = weights.get_mut(&name) {
                *weight /= (1 + count) as f64;
            }
        }
    }
}

/// Combines signals using Bayesian log-odds combination.
///
/// Converts each signal's confidence to log-odds, sums weighted log-odds
/// by direction, then converts back to probability.
///
/// # Arguments
/// * `signals` - Vector of (weight, SignalValue) pairs
///
/// # Returns
/// Combined SignalValue
pub fn combine_bayesian(signals: &[(f64, SignalValue)]) -> SignalValue {
    if signals.is_empty() {
        return SignalValue::neutral();
    }

    // Filter out neutral signals and zero-weight signals
    let directional_signals: Vec<_> = signals
        .iter()
        .filter(|(w, s)| *w > f64::EPSILON && s.direction != Direction::Neutral)
        .collect();

    if directional_signals.is_empty() {
        // Check if we have weighted neutral signals to compute average confidence
        let total_weight: f64 = signals.iter().map(|(w, _)| w).sum();
        if total_weight > f64::EPSILON {
            let avg_confidence: f64 =
                signals.iter().map(|(w, s)| w * s.confidence).sum::<f64>() / total_weight;
            return SignalValue::new(Direction::Neutral, 0.0, avg_confidence)
                .unwrap_or_else(|_| SignalValue::neutral());
        }
        return SignalValue::neutral();
    }

    let total_weight: f64 = directional_signals.iter().map(|(w, _)| w).sum();

    // Sum weighted log-odds for up direction
    // P(Up | signal) = confidence when direction is Up
    // P(Up | signal) = 1 - confidence when direction is Down
    let mut weighted_log_odds_sum = 0.0;

    for (weight, signal) in &directional_signals {
        // Base probability of up given this signal
        let p_up = match signal.direction {
            Direction::Up => {
                // Confidence represents how sure we are of "Up"
                // Map confidence [0, 1] to probability [0.5, 1.0]
                0.5 + 0.5 * signal.confidence
            }
            Direction::Down => {
                // Confidence represents how sure we are of "Down"
                // So P(Up) = 1 - P(Down) = 1 - (0.5 + 0.5 * confidence)
                0.5 - 0.5 * signal.confidence
            }
            Direction::Neutral => 0.5,
        };

        // Clamp to avoid log(0) or log(inf)
        let p_clamped = p_up.clamp(0.01, 0.99);

        // Convert to log-odds: log(p / (1 - p))
        let log_odds = (p_clamped / (1.0 - p_clamped)).ln();

        weighted_log_odds_sum += (weight / total_weight) * log_odds;
    }

    // Convert back to probability: p = exp(lo) / (1 + exp(lo))
    let combined_p = 1.0 / (1.0 + (-weighted_log_odds_sum).exp());

    // Determine direction and strength
    let direction = if combined_p > 0.55 {
        Direction::Up
    } else if combined_p < 0.45 {
        Direction::Down
    } else {
        Direction::Neutral
    };

    // Strength is distance from 0.5, normalized to [0, 1]
    let strength = ((combined_p - 0.5).abs() * 2.0).clamp(0.0, 1.0);

    // Confidence is based on the magnitude of combined probability
    let confidence = strength;

    SignalValue::new(direction, strength, confidence).unwrap_or_else(|_| SignalValue::neutral())
}

/// Combines multiple signal generators into a single composite signal.
///
/// The composite signal can use different methods to aggregate
/// the underlying signals, including weighted averaging, voting,
/// Bayesian combination, and strongest-signal selection.
pub struct CompositeSignal {
    /// Name of this composite signal
    name: String,
    /// Weight for this composite signal
    weight: f64,
    /// Underlying signal generators
    generators: Vec<Box<dyn SignalGenerator>>,
    /// Method for combining signals
    method: CombinationMethod,
    /// Enable multicollinearity adjustment
    pub adjust_multicollinearity: bool,
    /// Correlation threshold for multicollinearity adjustment (default 0.7)
    pub correlation_threshold: f64,
    /// Cached correlation matrix
    correlation_matrix: Option<CorrelationMatrix>,
}

impl CompositeSignal {
    /// Creates a new CompositeSignal with the specified combination method.
    #[must_use]
    pub fn new(name: impl Into<String>, method: CombinationMethod) -> Self {
        Self {
            name: name.into(),
            weight: 1.0,
            generators: Vec::new(),
            method,
            adjust_multicollinearity: false,
            correlation_threshold: 0.7,
            correlation_matrix: None,
        }
    }

    /// Creates a new CompositeSignal using weighted average combination.
    #[must_use]
    pub fn weighted_average(name: impl Into<String>) -> Self {
        Self::new(name, CombinationMethod::WeightedAverage)
    }

    /// Creates a new CompositeSignal using voting combination.
    #[must_use]
    pub fn voting(name: impl Into<String>) -> Self {
        Self::new(name, CombinationMethod::Voting)
    }

    /// Creates a new CompositeSignal using Bayesian combination.
    #[must_use]
    pub fn bayesian(name: impl Into<String>) -> Self {
        Self::new(name, CombinationMethod::Bayesian)
    }

    /// Creates a new CompositeSignal using RequireN combination.
    ///
    /// Only produces a directional signal when at least `min_agree` signals
    /// agree on the same direction. Neutral signals do not count toward agreement.
    #[must_use]
    pub fn require_n(name: impl Into<String>, min_agree: usize) -> Self {
        Self::new(name, CombinationMethod::RequireN { min_agree })
    }

    /// Builder method to use Bayesian combination.
    #[must_use]
    pub fn with_bayesian(mut self) -> Self {
        self.method = CombinationMethod::Bayesian;
        self
    }

    /// Builder method to require N signals to agree.
    ///
    /// Only produces a directional signal when at least `min_agree` signals
    /// agree on the same direction. Neutral signals do not count toward agreement.
    #[must_use]
    pub fn require_agreement(mut self, min_agree: usize) -> Self {
        self.method = CombinationMethod::RequireN { min_agree };
        self
    }

    /// Builder method to enable multicollinearity adjustment.
    #[must_use]
    pub fn with_multicollinearity_adjustment(mut self, threshold: f64) -> Self {
        self.adjust_multicollinearity = true;
        self.correlation_threshold = threshold;
        self
    }

    /// Sets the correlation matrix for multicollinearity adjustment.
    pub fn set_correlation_matrix(&mut self, matrix: CorrelationMatrix) {
        self.correlation_matrix = Some(matrix);
    }

    /// Adds a signal generator to this composite.
    pub fn add_generator(&mut self, generator: Box<dyn SignalGenerator>) {
        self.generators.push(generator);
    }

    /// Builder method to add a generator.
    #[must_use]
    pub fn with_generator(mut self, generator: Box<dyn SignalGenerator>) -> Self {
        self.generators.push(generator);
        self
    }

    /// Returns the number of generators in this composite.
    #[must_use]
    pub fn generator_count(&self) -> usize {
        self.generators.len()
    }

    /// Combines signals using weighted average.
    fn combine_weighted_average(&self, signals: &[(f64, SignalValue)]) -> SignalValue {
        if signals.is_empty() {
            return SignalValue::neutral();
        }

        let total_weight: f64 = signals.iter().map(|(w, _)| w).sum();
        if total_weight < f64::EPSILON {
            return SignalValue::neutral();
        }

        // Calculate weighted direction score: Up = +1, Down = -1, Neutral = 0
        let direction_score: f64 = signals
            .iter()
            .map(|(w, s)| {
                let dir_value = match s.direction {
                    Direction::Up => 1.0,
                    Direction::Down => -1.0,
                    Direction::Neutral => 0.0,
                };
                w * dir_value * s.strength
            })
            .sum::<f64>()
            / total_weight;

        // Calculate weighted average strength
        let avg_strength: f64 =
            signals.iter().map(|(w, s)| w * s.strength).sum::<f64>() / total_weight;

        // Calculate weighted average confidence
        let avg_confidence: f64 =
            signals.iter().map(|(w, s)| w * s.confidence).sum::<f64>() / total_weight;

        // Determine direction from score
        let direction = if direction_score > 0.1 {
            Direction::Up
        } else if direction_score < -0.1 {
            Direction::Down
        } else {
            Direction::Neutral
        };

        SignalValue::new(
            direction,
            avg_strength.clamp(0.0, 1.0),
            avg_confidence.clamp(0.0, 1.0),
        )
        .unwrap_or_else(|_| SignalValue::neutral())
    }

    /// Combines signals using majority voting.
    fn combine_voting(&self, signals: &[(f64, SignalValue)]) -> SignalValue {
        if signals.is_empty() {
            return SignalValue::neutral();
        }

        let mut up_votes = 0.0;
        let mut down_votes = 0.0;
        let mut total_strength = 0.0;
        let mut total_confidence = 0.0;

        for (weight, signal) in signals {
            match signal.direction {
                Direction::Up => up_votes += weight,
                Direction::Down => down_votes += weight,
                Direction::Neutral => {}
            }
            total_strength += weight * signal.strength;
            total_confidence += weight * signal.confidence;
        }

        let total_weight: f64 = signals.iter().map(|(w, _)| w).sum();
        let direction = if up_votes > down_votes {
            Direction::Up
        } else if down_votes > up_votes {
            Direction::Down
        } else {
            Direction::Neutral
        };

        let avg_strength = if total_weight > 0.0 {
            total_strength / total_weight
        } else {
            0.0
        };

        let avg_confidence = if total_weight > 0.0 {
            total_confidence / total_weight
        } else {
            0.0
        };

        SignalValue::new(
            direction,
            avg_strength.clamp(0.0, 1.0),
            avg_confidence.clamp(0.0, 1.0),
        )
        .unwrap_or_else(|_| SignalValue::neutral())
    }

    /// Combines signals by selecting the strongest.
    fn combine_strongest(&self, signals: &[(f64, SignalValue)]) -> SignalValue {
        signals
            .iter()
            .filter(|(_, s)| s.direction != Direction::Neutral)
            .max_by(|(w1, s1), (w2, s2)| {
                let score1 = w1 * s1.strength;
                let score2 = w2 * s2.strength;
                score1
                    .partial_cmp(&score2)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(_, s)| s.clone())
            .unwrap_or_else(SignalValue::neutral)
    }

    /// Combines signals using RequireN confirmation.
    ///
    /// Only returns a directional signal when at least `min_agree` signals
    /// agree on the same direction. Neutral signals are ignored.
    /// When both Up and Down reach `min_agree`, Up wins (tie-breaker).
    fn combine_require_n(&self, signals: &[(f64, SignalValue)], min_agree: usize) -> SignalValue {
        if signals.is_empty() {
            return SignalValue::neutral();
        }

        // Collect Up and Down signals separately
        let up_signals: Vec<&(f64, SignalValue)> = signals
            .iter()
            .filter(|(_, s)| s.direction == Direction::Up)
            .collect();

        let down_signals: Vec<&(f64, SignalValue)> = signals
            .iter()
            .filter(|(_, s)| s.direction == Direction::Down)
            .collect();

        let up_count = up_signals.len();
        let down_count = down_signals.len();

        // Check if either direction reaches min_agree
        let up_qualifies = up_count >= min_agree;
        let down_qualifies = down_count >= min_agree;

        // Determine winning direction (Up wins ties)
        let (direction, agreeing_signals): (Direction, Vec<&(f64, SignalValue)>) =
            if up_qualifies && down_qualifies {
                // Tie goes to Up
                (Direction::Up, up_signals)
            } else if up_qualifies {
                (Direction::Up, up_signals)
            } else if down_qualifies {
                (Direction::Down, down_signals)
            } else {
                // Neither qualifies
                return SignalValue::neutral();
            };

        // Calculate average strength and confidence of agreeing signals
        let count = agreeing_signals.len() as f64;
        let avg_strength = agreeing_signals
            .iter()
            .map(|(_, s)| s.strength)
            .sum::<f64>()
            / count;
        let avg_confidence = agreeing_signals
            .iter()
            .map(|(_, s)| s.confidence)
            .sum::<f64>()
            / count;

        SignalValue::new(
            direction,
            avg_strength.clamp(0.0, 1.0),
            avg_confidence.clamp(0.0, 1.0),
        )
        .unwrap_or_else(|_| SignalValue::neutral())
    }

    /// Applies multicollinearity adjustment to weights if enabled.
    fn apply_multicollinearity_adjustment(
        &self,
        signals: &mut [(f64, SignalValue)],
        generator_names: &[String],
    ) {
        if !self.adjust_multicollinearity {
            return;
        }

        if let Some(ref matrix) = self.correlation_matrix {
            // Build weight map
            let mut weights: HashMap<String, f64> = generator_names
                .iter()
                .zip(signals.iter().map(|(w, _)| *w))
                .map(|(n, w)| (n.clone(), w))
                .collect();

            // Adjust weights
            adjust_weights_for_multicollinearity(&mut weights, matrix, self.correlation_threshold);

            // Apply adjusted weights back
            for (i, name) in generator_names.iter().enumerate() {
                if let Some(&new_weight) = weights.get(name) {
                    signals[i].0 = new_weight;
                }
            }
        }
    }
}

#[async_trait]
impl SignalGenerator for CompositeSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        // Compute all underlying signals
        let mut signals = Vec::with_capacity(self.generators.len());
        let mut generator_names = Vec::with_capacity(self.generators.len());

        for generator in &mut self.generators {
            let signal = generator.compute(ctx).await?;
            let weight = generator.weight();
            signals.push((weight, signal));
            generator_names.push(generator.name().to_string());
        }

        // Apply multicollinearity adjustment if enabled
        self.apply_multicollinearity_adjustment(&mut signals, &generator_names);

        // Combine based on method
        let mut combined = match self.method {
            CombinationMethod::WeightedAverage => self.combine_weighted_average(&signals),
            CombinationMethod::Voting => self.combine_voting(&signals),
            CombinationMethod::Strongest => self.combine_strongest(&signals),
            CombinationMethod::Bayesian => combine_bayesian(&signals),
            CombinationMethod::RequireN { min_agree } => {
                self.combine_require_n(&signals, min_agree)
            }
        };

        // Enrich with per-generator metadata for factor analysis
        for (i, (weight, signal)) in signals.iter().enumerate() {
            let name = &generator_names[i];
            let dir_value = match signal.direction {
                Direction::Up => 1.0,
                Direction::Down => -1.0,
                Direction::Neutral => 0.0,
            };
            combined
                .metadata
                .insert(format!("{name}_direction"), dir_value);
            combined
                .metadata
                .insert(format!("{name}_strength"), signal.strength);
            combined
                .metadata
                .insert(format!("{name}_weight"), *weight);
        }

        Ok(combined)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn weight(&self) -> f64 {
        self.weight
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// Mock signal generator for testing
    struct MockGenerator {
        name: String,
        signal: SignalValue,
        weight: f64,
    }

    impl MockGenerator {
        fn new(name: &str, direction: Direction, strength: f64, weight: f64) -> Self {
            Self {
                name: name.to_string(),
                signal: SignalValue::new(direction, strength, 0.5).unwrap(),
                weight,
            }
        }

        fn with_confidence(
            name: &str,
            direction: Direction,
            strength: f64,
            confidence: f64,
            weight: f64,
        ) -> Self {
            Self {
                name: name.to_string(),
                signal: SignalValue::new(direction, strength, confidence).unwrap(),
                weight,
            }
        }
    }

    #[async_trait]
    impl SignalGenerator for MockGenerator {
        async fn compute(&mut self, _ctx: &SignalContext) -> Result<SignalValue> {
            Ok(self.signal.clone())
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn weight(&self) -> f64 {
            self.weight
        }
    }

    // ============================================
    // Existing tests (from before)
    // ============================================

    #[tokio::test]
    async fn composite_empty_returns_neutral() {
        let mut composite = CompositeSignal::weighted_average("test");
        let ctx = SignalContext::new(Utc::now(), "BTCUSD");

        let result = composite.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn composite_single_signal_passthrough() {
        let mut composite = CompositeSignal::weighted_average("test")
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.8, 1.0)));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Up);
        assert!((result.strength - 0.8).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn composite_weighted_average_combines() {
        let mut composite = CompositeSignal::weighted_average("test")
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.8, 2.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g2",
                Direction::Down,
                0.6,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // Up: 2.0 * 0.8 = 1.6, Down: 1.0 * 0.6 = 0.6
        // Direction score: (1.6 - 0.6) / 3.0 = 0.33 -> Up
        assert_eq!(result.direction, Direction::Up);
    }

    #[tokio::test]
    async fn composite_voting_majority_wins() {
        let mut composite = CompositeSignal::voting("test")
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.5, 1.0)))
            .with_generator(Box::new(MockGenerator::new("g2", Direction::Up, 0.5, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g3",
                Direction::Down,
                0.9,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // 2 Up votes vs 1 Down vote = Up wins
        assert_eq!(result.direction, Direction::Up);
    }

    #[tokio::test]
    async fn composite_strongest_wins() {
        let mut composite = CompositeSignal::new("test", CombinationMethod::Strongest)
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.3, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g2",
                Direction::Down,
                0.9,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // Down with 0.9 strength is strongest
        assert_eq!(result.direction, Direction::Down);
        assert!((result.strength - 0.9).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn composite_weighted_voting() {
        let mut composite = CompositeSignal::voting("test")
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.5, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g2",
                Direction::Down,
                0.5,
                3.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // Up: 1.0 vote, Down: 3.0 votes = Down wins
        assert_eq!(result.direction, Direction::Down);
    }

    #[tokio::test]
    async fn composite_neutral_signals_dont_vote() {
        let mut composite = CompositeSignal::voting("test")
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.5, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g2",
                Direction::Neutral,
                0.5,
                1.0,
            )))
            .with_generator(Box::new(MockGenerator::new(
                "g3",
                Direction::Neutral,
                0.5,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // 1 Up vote, 0 Down votes = Up wins
        assert_eq!(result.direction, Direction::Up);
    }

    #[test]
    fn composite_name_is_correct() {
        let composite = CompositeSignal::weighted_average("my_composite");
        assert_eq!(composite.name(), "my_composite");
    }

    #[test]
    fn composite_generator_count() {
        let composite = CompositeSignal::weighted_average("test")
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.5, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g2",
                Direction::Down,
                0.5,
                1.0,
            )));

        assert_eq!(composite.generator_count(), 2);
    }

    #[tokio::test]
    async fn composite_balanced_signals_neutral() {
        let mut composite = CompositeSignal::weighted_average("test")
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.5, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g2",
                Direction::Down,
                0.5,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // Equal and opposite = Neutral
        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn composite_all_neutral_returns_neutral() {
        let mut composite = CompositeSignal::voting("test")
            .with_generator(Box::new(MockGenerator::new(
                "g1",
                Direction::Neutral,
                0.5,
                1.0,
            )))
            .with_generator(Box::new(MockGenerator::new(
                "g2",
                Direction::Neutral,
                0.5,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
    }

    // ============================================
    // Phase 2F: Bayesian Combination Tests
    // ============================================

    #[test]
    fn bayesian_high_confidence_up_gives_high_prob() {
        // High confidence Up signals should give Up direction
        let signals = vec![
            (1.0, SignalValue::new(Direction::Up, 0.9, 0.9).unwrap()),
            (1.0, SignalValue::new(Direction::Up, 0.8, 0.85).unwrap()),
        ];

        let result = combine_bayesian(&signals);

        assert_eq!(result.direction, Direction::Up);
        assert!(result.strength > 0.5, "strength was {}", result.strength);
        assert!(
            result.confidence > 0.5,
            "confidence was {}",
            result.confidence
        );
    }

    #[test]
    fn bayesian_high_confidence_down_gives_low_prob() {
        // High confidence Down signals should give Down direction
        let signals = vec![
            (1.0, SignalValue::new(Direction::Down, 0.9, 0.9).unwrap()),
            (1.0, SignalValue::new(Direction::Down, 0.8, 0.85).unwrap()),
        ];

        let result = combine_bayesian(&signals);

        assert_eq!(result.direction, Direction::Down);
        assert!(result.strength > 0.5, "strength was {}", result.strength);
    }

    #[test]
    fn bayesian_mixed_signals_give_uncertain() {
        // Conflicting high-confidence signals should give uncertain result
        let signals = vec![
            (1.0, SignalValue::new(Direction::Up, 0.8, 0.8).unwrap()),
            (1.0, SignalValue::new(Direction::Down, 0.8, 0.8).unwrap()),
        ];

        let result = combine_bayesian(&signals);

        // Should be neutral or very weak direction
        assert!(
            result.direction == Direction::Neutral || result.strength < 0.3,
            "direction={:?}, strength={}",
            result.direction,
            result.strength
        );
    }

    #[test]
    fn bayesian_handles_neutral_signals() {
        // Neutral signals should result in neutral output
        let signals = vec![
            (1.0, SignalValue::new(Direction::Neutral, 0.0, 0.5).unwrap()),
            (1.0, SignalValue::new(Direction::Neutral, 0.0, 0.5).unwrap()),
        ];

        let result = combine_bayesian(&signals);

        assert_eq!(result.direction, Direction::Neutral);
    }

    #[test]
    fn bayesian_weights_affect_outcome() {
        // Higher weighted signal should dominate
        let signals = vec![
            (3.0, SignalValue::new(Direction::Up, 0.8, 0.8).unwrap()),
            (1.0, SignalValue::new(Direction::Down, 0.8, 0.8).unwrap()),
        ];

        let result = combine_bayesian(&signals);

        // Up signal has 3x weight, so should win
        assert_eq!(result.direction, Direction::Up);
    }

    #[test]
    fn bayesian_empty_signals_returns_neutral() {
        let signals: Vec<(f64, SignalValue)> = vec![];
        let result = combine_bayesian(&signals);
        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn composite_bayesian_method_works() {
        let mut composite = CompositeSignal::bayesian("test")
            .with_generator(Box::new(MockGenerator::with_confidence(
                "g1",
                Direction::Up,
                0.8,
                0.9,
                1.0,
            )))
            .with_generator(Box::new(MockGenerator::with_confidence(
                "g2",
                Direction::Up,
                0.7,
                0.85,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Up);
    }

    #[test]
    fn composite_with_bayesian_builder() {
        let composite = CompositeSignal::weighted_average("test").with_bayesian();
        assert_eq!(composite.method, CombinationMethod::Bayesian);
    }

    // ============================================
    // Phase 2F: Correlation Matrix Tests
    // ============================================

    #[test]
    fn correlation_matrix_stores_values() {
        let mut matrix = CorrelationMatrix::new(vec!["sig1".into(), "sig2".into(), "sig3".into()]);

        matrix.set(0, 1, 0.8);
        matrix.set(1, 2, -0.5);

        assert!((matrix.get(0, 1) - 0.8).abs() < f64::EPSILON);
        assert!((matrix.get(1, 2) - (-0.5)).abs() < f64::EPSILON);
    }

    #[test]
    fn correlation_matrix_symmetric() {
        let mut matrix = CorrelationMatrix::new(vec!["sig1".into(), "sig2".into()]);

        matrix.set(0, 1, 0.75);

        // Should be symmetric
        assert!((matrix.get(0, 1) - 0.75).abs() < f64::EPSILON);
        assert!((matrix.get(1, 0) - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn correlation_matrix_diagonal_is_one() {
        let matrix = CorrelationMatrix::new(vec!["sig1".into(), "sig2".into(), "sig3".into()]);

        assert!((matrix.get(0, 0) - 1.0).abs() < f64::EPSILON);
        assert!((matrix.get(1, 1) - 1.0).abs() < f64::EPSILON);
        assert!((matrix.get(2, 2) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn correlation_matrix_get_by_name() {
        let mut matrix = CorrelationMatrix::new(vec!["alpha".into(), "beta".into()]);
        matrix.set(0, 1, 0.6);

        assert_eq!(matrix.get_by_name("alpha", "beta"), Some(0.6));
        assert_eq!(matrix.get_by_name("beta", "alpha"), Some(0.6));
        assert!(matrix.get_by_name("alpha", "gamma").is_none());
    }

    #[test]
    fn correlation_matrix_clamps_values() {
        let mut matrix = CorrelationMatrix::new(vec!["a".into(), "b".into()]);
        matrix.set(0, 1, 1.5); // Over 1.0
        assert!((matrix.get(0, 1) - 1.0).abs() < f64::EPSILON);

        matrix.set(0, 1, -1.5); // Under -1.0
        assert!((matrix.get(0, 1) - (-1.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn calculate_correlation_perfect_positive() {
        // Signals that move together
        let historical = vec![
            {
                let mut m = HashMap::new();
                m.insert(
                    "sig1".to_string(),
                    SignalValue::new(Direction::Up, 0.8, 0.5).unwrap(),
                );
                m.insert(
                    "sig2".to_string(),
                    SignalValue::new(Direction::Up, 0.8, 0.5).unwrap(),
                );
                m
            },
            {
                let mut m = HashMap::new();
                m.insert(
                    "sig1".to_string(),
                    SignalValue::new(Direction::Down, 0.6, 0.5).unwrap(),
                );
                m.insert(
                    "sig2".to_string(),
                    SignalValue::new(Direction::Down, 0.6, 0.5).unwrap(),
                );
                m
            },
            {
                let mut m = HashMap::new();
                m.insert(
                    "sig1".to_string(),
                    SignalValue::new(Direction::Up, 0.4, 0.5).unwrap(),
                );
                m.insert(
                    "sig2".to_string(),
                    SignalValue::new(Direction::Up, 0.4, 0.5).unwrap(),
                );
                m
            },
        ];

        let names = vec!["sig1".to_string(), "sig2".to_string()];
        let matrix = calculate_correlation_matrix(&historical, &names);

        let corr = matrix.get(0, 1);
        assert!(corr > 0.99, "correlation was {corr}");
    }

    #[test]
    fn calculate_correlation_perfect_negative() {
        // Signals that move opposite
        let historical = vec![
            {
                let mut m = HashMap::new();
                m.insert(
                    "sig1".to_string(),
                    SignalValue::new(Direction::Up, 0.8, 0.5).unwrap(),
                );
                m.insert(
                    "sig2".to_string(),
                    SignalValue::new(Direction::Down, 0.8, 0.5).unwrap(),
                );
                m
            },
            {
                let mut m = HashMap::new();
                m.insert(
                    "sig1".to_string(),
                    SignalValue::new(Direction::Down, 0.6, 0.5).unwrap(),
                );
                m.insert(
                    "sig2".to_string(),
                    SignalValue::new(Direction::Up, 0.6, 0.5).unwrap(),
                );
                m
            },
            {
                let mut m = HashMap::new();
                m.insert(
                    "sig1".to_string(),
                    SignalValue::new(Direction::Up, 0.4, 0.5).unwrap(),
                );
                m.insert(
                    "sig2".to_string(),
                    SignalValue::new(Direction::Down, 0.4, 0.5).unwrap(),
                );
                m
            },
        ];

        let names = vec!["sig1".to_string(), "sig2".to_string()];
        let matrix = calculate_correlation_matrix(&historical, &names);

        let corr = matrix.get(0, 1);
        assert!(corr < -0.99, "correlation was {corr}");
    }

    #[test]
    fn calculate_correlation_uncorrelated() {
        // Signals with no clear relationship
        let historical = vec![
            {
                let mut m = HashMap::new();
                m.insert(
                    "sig1".to_string(),
                    SignalValue::new(Direction::Up, 0.5, 0.5).unwrap(),
                );
                m.insert(
                    "sig2".to_string(),
                    SignalValue::new(Direction::Down, 0.3, 0.5).unwrap(),
                );
                m
            },
            {
                let mut m = HashMap::new();
                m.insert(
                    "sig1".to_string(),
                    SignalValue::new(Direction::Down, 0.7, 0.5).unwrap(),
                );
                m.insert(
                    "sig2".to_string(),
                    SignalValue::new(Direction::Up, 0.5, 0.5).unwrap(),
                );
                m
            },
            {
                let mut m = HashMap::new();
                m.insert(
                    "sig1".to_string(),
                    SignalValue::new(Direction::Up, 0.3, 0.5).unwrap(),
                );
                m.insert(
                    "sig2".to_string(),
                    SignalValue::new(Direction::Up, 0.6, 0.5).unwrap(),
                );
                m
            },
            {
                let mut m = HashMap::new();
                m.insert(
                    "sig1".to_string(),
                    SignalValue::new(Direction::Down, 0.4, 0.5).unwrap(),
                );
                m.insert(
                    "sig2".to_string(),
                    SignalValue::new(Direction::Down, 0.2, 0.5).unwrap(),
                );
                m
            },
        ];

        let names = vec!["sig1".to_string(), "sig2".to_string()];
        let matrix = calculate_correlation_matrix(&historical, &names);

        let corr = matrix.get(0, 1);
        // For this random-ish data, correlation should not be extreme
        assert!(corr.abs() < 0.9, "correlation was {corr}");
    }

    // ============================================
    // Phase 2F: Weight Adjustment Tests
    // ============================================

    #[test]
    fn weight_adjustment_reduces_correlated() {
        let mut matrix = CorrelationMatrix::new(vec!["a".into(), "b".into(), "c".into()]);
        // a and b are highly correlated
        matrix.set(0, 1, 0.85);
        // c is uncorrelated with both
        matrix.set(0, 2, 0.2);
        matrix.set(1, 2, 0.1);

        let mut weights = HashMap::new();
        weights.insert("a".to_string(), 1.0);
        weights.insert("b".to_string(), 1.0);
        weights.insert("c".to_string(), 1.0);

        adjust_weights_for_multicollinearity(&mut weights, &matrix, 0.7);

        // a and b should have reduced weights (each has 1 high correlation)
        assert!(weights["a"] < 1.0, "a weight was {}", weights["a"]);
        assert!(weights["b"] < 1.0, "b weight was {}", weights["b"]);
        // c should remain unchanged
        assert!((weights["c"] - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn weight_adjustment_keeps_uncorrelated() {
        let matrix = CorrelationMatrix::new(vec!["a".into(), "b".into()]);
        // Default correlations are 0.0 (except diagonal)

        let mut weights = HashMap::new();
        weights.insert("a".to_string(), 1.0);
        weights.insert("b".to_string(), 1.0);

        adjust_weights_for_multicollinearity(&mut weights, &matrix, 0.7);

        // No high correlations, weights unchanged
        assert!((weights["a"] - 1.0).abs() < f64::EPSILON);
        assert!((weights["b"] - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn weight_adjustment_multiple_correlations() {
        let mut matrix = CorrelationMatrix::new(vec!["a".into(), "b".into(), "c".into()]);
        // a is correlated with both b and c
        matrix.set(0, 1, 0.8);
        matrix.set(0, 2, 0.75);
        // b and c not correlated with each other
        matrix.set(1, 2, 0.1);

        let mut weights = HashMap::new();
        weights.insert("a".to_string(), 1.0);
        weights.insert("b".to_string(), 1.0);
        weights.insert("c".to_string(), 1.0);

        adjust_weights_for_multicollinearity(&mut weights, &matrix, 0.7);

        // a has 2 high correlations, so weight = 1.0 / (1 + 2) = 0.333...
        assert!(
            (weights["a"] - 1.0 / 3.0).abs() < 0.01,
            "a weight was {}",
            weights["a"]
        );
        // b has 1 high correlation, so weight = 1.0 / (1 + 1) = 0.5
        assert!(
            (weights["b"] - 0.5).abs() < 0.01,
            "b weight was {}",
            weights["b"]
        );
        // c has 1 high correlation, so weight = 0.5
        assert!(
            (weights["c"] - 0.5).abs() < 0.01,
            "c weight was {}",
            weights["c"]
        );
    }

    #[tokio::test]
    async fn composite_with_multicollinearity_adjustment() {
        let mut matrix = CorrelationMatrix::new(vec!["g1".into(), "g2".into()]);
        matrix.set(0, 1, 0.9); // Highly correlated

        let mut composite = CompositeSignal::weighted_average("test")
            .with_multicollinearity_adjustment(0.7)
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.8, 1.0)))
            .with_generator(Box::new(MockGenerator::new("g2", Direction::Up, 0.7, 1.0)));

        composite.set_correlation_matrix(matrix);

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // Should still compute - weights are adjusted internally
        assert_eq!(result.direction, Direction::Up);
    }

    #[test]
    fn composite_multicollinearity_builder() {
        let composite =
            CompositeSignal::weighted_average("test").with_multicollinearity_adjustment(0.8);

        assert!(composite.adjust_multicollinearity);
        assert!((composite.correlation_threshold - 0.8).abs() < f64::EPSILON);
    }

    // ============================================
    // Phase 2.2E: RequireN Confirmation Tests
    // ============================================

    #[test]
    fn require_n_variant_exists() {
        // Test that the RequireN variant exists and can be constructed
        let method = CombinationMethod::RequireN { min_agree: 2 };
        match method {
            CombinationMethod::RequireN { min_agree } => {
                assert_eq!(min_agree, 2);
            }
            _ => panic!("Expected RequireN variant"),
        }
    }

    #[tokio::test]
    async fn require_n_returns_neutral_when_insufficient_agreement() {
        // Only 1 Up signal, but require 2 to agree
        let mut composite = CompositeSignal::require_n("test", 2)
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.8, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g2",
                Direction::Down,
                0.6,
                1.0,
            )))
            .with_generator(Box::new(MockGenerator::new(
                "g3",
                Direction::Neutral,
                0.5,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // Only 1 Up, 1 Down - neither reaches min_agree=2
        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn require_n_returns_direction_when_enough_agree() {
        // 2 Up signals, require 2 to agree
        let mut composite = CompositeSignal::require_n("test", 2)
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.8, 1.0)))
            .with_generator(Box::new(MockGenerator::new("g2", Direction::Up, 0.7, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g3",
                Direction::Down,
                0.6,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // 2 Up signals meet the threshold
        assert_eq!(result.direction, Direction::Up);
    }

    #[tokio::test]
    async fn require_n_requires_all_with_max_value() {
        // Require all 3 signals to agree
        let mut composite = CompositeSignal::require_n("test", 3)
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.8, 1.0)))
            .with_generator(Box::new(MockGenerator::new("g2", Direction::Up, 0.7, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g3",
                Direction::Down,
                0.6,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // Only 2 Up, need 3 - return Neutral
        assert_eq!(result.direction, Direction::Neutral);

        // Now test with all 3 agreeing
        let mut composite_all = CompositeSignal::require_n("test", 3)
            .with_generator(Box::new(MockGenerator::new(
                "g1",
                Direction::Down,
                0.8,
                1.0,
            )))
            .with_generator(Box::new(MockGenerator::new(
                "g2",
                Direction::Down,
                0.7,
                1.0,
            )))
            .with_generator(Box::new(MockGenerator::new(
                "g3",
                Direction::Down,
                0.6,
                1.0,
            )));

        let result_all = composite_all.compute(&ctx).await.unwrap();
        assert_eq!(result_all.direction, Direction::Down);
    }

    #[tokio::test]
    async fn require_n_neutral_signals_dont_count() {
        // 2 Neutral signals should not count toward agreement
        let mut composite = CompositeSignal::require_n("test", 2)
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.8, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g2",
                Direction::Neutral,
                0.5,
                1.0,
            )))
            .with_generator(Box::new(MockGenerator::new(
                "g3",
                Direction::Neutral,
                0.5,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // Only 1 Up, 2 Neutral (which don't count) - not enough agreement
        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn require_n_averages_strength_of_agreeing_signals() {
        // Strength should be average of agreeing signals only
        let mut composite = CompositeSignal::require_n("test", 2)
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.8, 1.0)))
            .with_generator(Box::new(MockGenerator::new("g2", Direction::Up, 0.6, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g3",
                Direction::Down,
                0.9,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Up);
        // Average of 0.8 and 0.6 = 0.7
        assert!(
            (result.strength - 0.7).abs() < 0.01,
            "strength was {}",
            result.strength
        );
    }

    #[tokio::test]
    async fn require_n_averages_confidence_of_agreeing_signals() {
        // Confidence should be average of agreeing signals only
        let mut composite = CompositeSignal::require_n("test", 2)
            .with_generator(Box::new(MockGenerator::with_confidence(
                "g1",
                Direction::Down,
                0.5,
                0.9,
                1.0,
            )))
            .with_generator(Box::new(MockGenerator::with_confidence(
                "g2",
                Direction::Down,
                0.5,
                0.7,
                1.0,
            )))
            .with_generator(Box::new(MockGenerator::with_confidence(
                "g3",
                Direction::Up,
                0.5,
                0.5,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Down);
        // Average of 0.9 and 0.7 = 0.8
        assert!(
            (result.confidence - 0.8).abs() < 0.01,
            "confidence was {}",
            result.confidence
        );
    }

    #[test]
    fn require_agreement_builder_sets_method() {
        let composite = CompositeSignal::weighted_average("test").require_agreement(3);

        match composite.method {
            CombinationMethod::RequireN { min_agree } => {
                assert_eq!(min_agree, 3);
            }
            _ => panic!("Expected RequireN method"),
        }
    }

    #[tokio::test]
    async fn require_n_empty_signals_returns_neutral() {
        let mut composite = CompositeSignal::require_n("test", 2);

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Neutral);
    }

    #[tokio::test]
    async fn require_n_tie_goes_to_up() {
        // When both Up and Down reach min_agree, Up wins
        let mut composite = CompositeSignal::require_n("test", 2)
            .with_generator(Box::new(MockGenerator::new("g1", Direction::Up, 0.8, 1.0)))
            .with_generator(Box::new(MockGenerator::new("g2", Direction::Up, 0.7, 1.0)))
            .with_generator(Box::new(MockGenerator::new(
                "g3",
                Direction::Down,
                0.8,
                1.0,
            )))
            .with_generator(Box::new(MockGenerator::new(
                "g4",
                Direction::Down,
                0.7,
                1.0,
            )));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = composite.compute(&ctx).await.unwrap();

        // Both have 2 votes, tie goes to Up
        assert_eq!(result.direction, Direction::Up);
    }
}
