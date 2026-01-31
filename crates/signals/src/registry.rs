//! Signal registry for managing multiple signal generators.
//!
//! The registry provides a centralized way to register, retrieve, and compute
//! signals from multiple signal generators.

use std::collections::HashMap;

use algo_trade_core::{SignalContext, SignalGenerator, SignalValue};
use anyhow::Result;

/// Registry for managing signal generators.
///
/// Provides a centralized way to register signal generators and compute
/// all signals at once. Thread-safe for concurrent access.
pub struct SignalRegistry {
    generators: HashMap<String, Box<dyn SignalGenerator>>,
}

impl Default for SignalRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalRegistry {
    /// Creates a new empty signal registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            generators: HashMap::new(),
        }
    }

    /// Registers a signal generator.
    ///
    /// If a generator with the same name already exists, it will be replaced.
    pub fn register(&mut self, generator: Box<dyn SignalGenerator>) {
        let name = generator.name().to_string();
        self.generators.insert(name, generator);
    }

    /// Registers a signal generator with a custom name.
    ///
    /// This allows registering the same generator type with different names,
    /// useful for testing variants of the same signal with different configurations.
    pub fn register_with_name(&mut self, generator: Box<dyn SignalGenerator>, name: &str) {
        self.generators.insert(name.to_string(), generator);
    }

    /// Returns a reference to a signal generator by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn SignalGenerator> {
        self.generators.get(name).map(|b| b.as_ref())
    }

    /// Returns a mutable reference to a boxed signal generator by name.
    #[must_use]
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Box<dyn SignalGenerator>> {
        self.generators.get_mut(name)
    }

    /// Returns true if the registry contains a generator with the given name.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.generators.contains_key(name)
    }

    /// Returns the names of all registered signal generators.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.generators.keys().map(|s| s.as_str()).collect()
    }

    /// Returns the number of registered generators.
    #[must_use]
    pub fn len(&self) -> usize {
        self.generators.len()
    }

    /// Returns true if no generators are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.generators.is_empty()
    }

    /// Removes a signal generator by name.
    ///
    /// Returns the removed generator, or None if not found.
    pub fn remove(&mut self, name: &str) -> Option<Box<dyn SignalGenerator>> {
        self.generators.remove(name)
    }

    /// Clears all registered generators.
    pub fn clear(&mut self) {
        self.generators.clear();
    }

    /// Computes all signals and returns the results.
    ///
    /// Returns a map from signal name to computed SignalValue.
    /// If a signal fails to compute, it will be absent from the results
    /// and the error will be logged.
    ///
    /// # Errors
    /// Does not return errors for individual signal failures; instead,
    /// those signals are skipped. Only returns an error if a critical
    /// system failure occurs.
    pub async fn compute_all(
        &mut self,
        ctx: &SignalContext,
    ) -> Result<HashMap<String, SignalValue>> {
        let mut results = HashMap::with_capacity(self.generators.len());

        for (name, generator) in &mut self.generators {
            match generator.compute(ctx).await {
                Ok(value) => {
                    results.insert(name.clone(), value);
                }
                Err(e) => {
                    tracing::warn!(
                        signal = %name,
                        error = %e,
                        "Signal computation failed, skipping"
                    );
                }
            }
        }

        Ok(results)
    }

    /// Computes signals and returns results with errors.
    ///
    /// Unlike `compute_all`, this method returns errors alongside successful results.
    ///
    /// # Errors
    /// Does not propagate errors; all errors are captured in the returned map.
    pub async fn compute_all_with_errors(
        &mut self,
        ctx: &SignalContext,
    ) -> HashMap<String, Result<SignalValue>> {
        let mut results = HashMap::with_capacity(self.generators.len());

        for (name, generator) in &mut self.generators {
            let result = generator.compute(ctx).await;
            results.insert(name.clone(), result);
        }

        results
    }

    /// Computes a single signal by name.
    ///
    /// # Errors
    /// Returns an error if the signal is not found or computation fails.
    pub async fn compute_one(&mut self, name: &str, ctx: &SignalContext) -> Result<SignalValue> {
        let generator = self
            .generators
            .get_mut(name)
            .ok_or_else(|| anyhow::anyhow!("Signal generator '{}' not found", name))?;

        generator.compute(ctx).await
    }

    /// Returns an iterator over registered generator names and their weights.
    pub fn weights(&self) -> impl Iterator<Item = (&str, f64)> {
        self.generators
            .iter()
            .map(|(name, gen)| (name.as_str(), gen.weight()))
    }
}

impl std::fmt::Debug for SignalRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignalRegistry")
            .field("generators", &self.names())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use algo_trade_core::Direction;
    use async_trait::async_trait;
    use chrono::Utc;

    // Mock signal generator for testing
    struct MockSignal {
        name: String,
        weight: f64,
        should_fail: bool,
    }

    impl MockSignal {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                weight: 1.0,
                should_fail: false,
            }
        }

        fn with_weight(mut self, weight: f64) -> Self {
            self.weight = weight;
            self
        }

        fn failing(mut self) -> Self {
            self.should_fail = true;
            self
        }
    }

    #[async_trait]
    impl SignalGenerator for MockSignal {
        async fn compute(&mut self, _ctx: &SignalContext) -> Result<SignalValue> {
            if self.should_fail {
                anyhow::bail!("Mock signal failure");
            }
            SignalValue::new(Direction::Up, 0.5, 0.5)
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn weight(&self) -> f64 {
            self.weight
        }
    }

    // ============================================
    // Registry Creation Tests
    // ============================================

    #[test]
    fn registry_new_is_empty() {
        let registry = SignalRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn registry_default_is_empty() {
        let registry = SignalRegistry::default();
        assert!(registry.is_empty());
    }

    // ============================================
    // Registration Tests
    // ============================================

    #[test]
    fn registry_registers_and_retrieves_signals() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("signal_a")));
        registry.register(Box::new(MockSignal::new("signal_b")));

        assert_eq!(registry.len(), 2);
        assert!(registry.contains("signal_a"));
        assert!(registry.contains("signal_b"));
        assert!(!registry.contains("signal_c"));

        let signal_a = registry.get("signal_a");
        assert!(signal_a.is_some());
        assert_eq!(signal_a.unwrap().name(), "signal_a");
    }

    #[test]
    fn registry_replaces_signal_with_same_name() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("signal_a").with_weight(1.0)));
        registry.register(Box::new(MockSignal::new("signal_a").with_weight(2.0)));

        assert_eq!(registry.len(), 1);

        let signal = registry.get("signal_a").unwrap();
        assert!((signal.weight() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn registry_names_returns_all_names() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("alpha")));
        registry.register(Box::new(MockSignal::new("beta")));
        registry.register(Box::new(MockSignal::new("gamma")));

        let names = registry.names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        assert!(names.contains(&"gamma"));
    }

    #[test]
    fn registry_handles_empty_state() {
        let registry = SignalRegistry::new();

        assert!(registry.get("nonexistent").is_none());
        assert!(registry.names().is_empty());
        assert!(registry.is_empty());
    }

    // ============================================
    // Removal Tests
    // ============================================

    #[test]
    fn registry_removes_signal() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("signal_a")));
        registry.register(Box::new(MockSignal::new("signal_b")));

        let removed = registry.remove("signal_a");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().name(), "signal_a");

        assert_eq!(registry.len(), 1);
        assert!(!registry.contains("signal_a"));
        assert!(registry.contains("signal_b"));
    }

    #[test]
    fn registry_remove_nonexistent_returns_none() {
        let mut registry = SignalRegistry::new();
        assert!(registry.remove("nonexistent").is_none());
    }

    #[test]
    fn registry_clear_removes_all() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("signal_a")));
        registry.register(Box::new(MockSignal::new("signal_b")));

        registry.clear();

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    // ============================================
    // Compute Tests
    // ============================================

    #[tokio::test]
    async fn registry_compute_all_returns_all_signals() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("signal_a")));
        registry.register(Box::new(MockSignal::new("signal_b")));
        registry.register(Box::new(MockSignal::new("signal_c")));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let results = registry.compute_all(&ctx).await.unwrap();

        assert_eq!(results.len(), 3);
        assert!(results.contains_key("signal_a"));
        assert!(results.contains_key("signal_b"));
        assert!(results.contains_key("signal_c"));

        // Verify signal values
        for (_, value) in &results {
            assert_eq!(value.direction, Direction::Up);
            assert!((value.strength - 0.5).abs() < f64::EPSILON);
        }
    }

    #[tokio::test]
    async fn registry_compute_all_skips_failing_signals() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("good_signal")));
        registry.register(Box::new(MockSignal::new("bad_signal").failing()));
        registry.register(Box::new(MockSignal::new("another_good")));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let results = registry.compute_all(&ctx).await.unwrap();

        // Only good signals should be in results
        assert_eq!(results.len(), 2);
        assert!(results.contains_key("good_signal"));
        assert!(results.contains_key("another_good"));
        assert!(!results.contains_key("bad_signal"));
    }

    #[tokio::test]
    async fn registry_compute_all_with_errors_captures_failures() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("good_signal")));
        registry.register(Box::new(MockSignal::new("bad_signal").failing()));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let results = registry.compute_all_with_errors(&ctx).await;

        assert_eq!(results.len(), 2);

        assert!(results.get("good_signal").unwrap().is_ok());
        assert!(results.get("bad_signal").unwrap().is_err());
    }

    #[tokio::test]
    async fn registry_compute_all_empty_returns_empty_map() {
        let mut registry = SignalRegistry::new();

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let results = registry.compute_all(&ctx).await.unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn registry_compute_one_returns_single_signal() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("target")));
        registry.register(Box::new(MockSignal::new("other")));

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = registry.compute_one("target", &ctx).await.unwrap();

        assert_eq!(result.direction, Direction::Up);
    }

    #[tokio::test]
    async fn registry_compute_one_not_found_returns_error() {
        let mut registry = SignalRegistry::new();

        let ctx = SignalContext::new(Utc::now(), "BTCUSD");
        let result = registry.compute_one("nonexistent", &ctx).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    // ============================================
    // Weight Tests
    // ============================================

    #[test]
    fn registry_weights_returns_all_weights() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("signal_a").with_weight(1.0)));
        registry.register(Box::new(MockSignal::new("signal_b").with_weight(2.0)));
        registry.register(Box::new(MockSignal::new("signal_c").with_weight(0.5)));

        let weights: HashMap<&str, f64> = registry.weights().collect();

        assert_eq!(weights.len(), 3);
        assert!((weights["signal_a"] - 1.0).abs() < f64::EPSILON);
        assert!((weights["signal_b"] - 2.0).abs() < f64::EPSILON);
        assert!((weights["signal_c"] - 0.5).abs() < f64::EPSILON);
    }

    // ============================================
    // Debug Tests
    // ============================================

    #[test]
    fn registry_debug_shows_names() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("alpha")));
        registry.register(Box::new(MockSignal::new("beta")));

        let debug_str = format!("{:?}", registry);

        assert!(debug_str.contains("SignalRegistry"));
        assert!(debug_str.contains("generators"));
    }

    // ============================================
    // Mutable Access Tests
    // ============================================

    #[test]
    fn registry_get_mut_allows_modification() {
        let mut registry = SignalRegistry::new();

        registry.register(Box::new(MockSignal::new("signal_a").with_weight(1.0)));

        // Verify we can get mutable access
        let signal = registry.get_mut("signal_a");
        assert!(signal.is_some());

        // Verify we can still access after mutable borrow ends
        assert!(registry.contains("signal_a"));
    }
}
