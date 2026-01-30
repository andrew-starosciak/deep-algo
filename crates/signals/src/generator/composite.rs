//! Composite signal generator.
//!
//! Combines multiple signal generators into a unified signal using
//! various aggregation methods.

use algo_trade_core::{Direction, SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;
use async_trait::async_trait;

/// Method for combining multiple signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombinationMethod {
    /// Weighted average of signal directions and strengths
    WeightedAverage,
    /// Majority vote on direction
    Voting,
    /// Use the strongest signal
    Strongest,
}

/// Combines multiple signal generators into a single composite signal.
///
/// The composite signal can use different methods to aggregate
/// the underlying signals, including weighted averaging, voting,
/// and strongest-signal selection.
pub struct CompositeSignal {
    /// Name of this composite signal
    name: String,
    /// Underlying signal generators
    generators: Vec<Box<dyn SignalGenerator>>,
    /// Method for combining signals
    method: CombinationMethod,
}

impl CompositeSignal {
    /// Creates a new CompositeSignal with the specified combination method.
    #[must_use]
    pub fn new(name: impl Into<String>, method: CombinationMethod) -> Self {
        Self {
            name: name.into(),
            generators: Vec::new(),
            method,
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
}

#[async_trait]
impl SignalGenerator for CompositeSignal {
    async fn compute(&mut self, ctx: &SignalContext) -> Result<SignalValue> {
        // Compute all underlying signals
        let mut signals = Vec::with_capacity(self.generators.len());

        for generator in &mut self.generators {
            let signal = generator.compute(ctx).await?;
            let weight = generator.weight();
            signals.push((weight, signal));
        }

        // Combine based on method
        let combined = match self.method {
            CombinationMethod::WeightedAverage => self.combine_weighted_average(&signals),
            CombinationMethod::Voting => self.combine_voting(&signals),
            CombinationMethod::Strongest => self.combine_strongest(&signals),
        };

        Ok(combined)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn weight(&self) -> f64 {
        1.0
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
}
