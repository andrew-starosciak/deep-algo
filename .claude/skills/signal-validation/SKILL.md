---
name: signal-validation
description: Statistical validation patterns for trading signals. Hypothesis testing, confidence intervals, and Go/No-Go criteria.
---

# Signal Validation

Statistical methods for validating trading signals before deployment.

## When to Activate

- Developing new trading signals
- Evaluating signal predictive power
- Running backtests and analyzing results
- Making Go/No-Go decisions on signals

## Core Principle

**No signal goes live without statistical validation.** Every signal must demonstrate:
- p-value < 0.05 (production) or < 0.10 (development)
- Sufficient sample size (n > 100)
- Positive expected value after fees

## Validation Framework

### Signal Validation Struct

```rust
pub struct SignalValidation {
    pub signal_name: String,
    pub sample_size: usize,
    pub wins: usize,
    pub win_rate: f64,
    pub wilson_ci: (f64, f64),
    pub binomial_p_value: f64,
    pub information_coefficient: f64,
    pub conditional_probability: f64,  // P(Up | signal > threshold)
}

impl SignalValidation {
    pub fn is_valid_for_production(&self) -> bool {
        self.sample_size >= 100
            && self.binomial_p_value < 0.05
            && self.win_rate > 0.52
    }

    pub fn is_valid_for_development(&self) -> bool {
        self.sample_size >= 50
            && self.binomial_p_value < 0.10
    }
}
```

## Statistical Methods

### Wilson Score Confidence Interval

For win rate estimation with proper handling of small samples:

```rust
/// Calculate Wilson score confidence interval
/// Returns (lower, upper) bounds at given confidence level
pub fn wilson_ci(wins: usize, total: usize, z: f64) -> (f64, f64) {
    let n = total as f64;
    let p = wins as f64 / n;

    let denominator = 1.0 + z.powi(2) / n;
    let center = p + z.powi(2) / (2.0 * n);
    let spread = z * (p * (1.0 - p) / n + z.powi(2) / (4.0 * n.powi(2))).sqrt();

    (
        (center - spread) / denominator,
        (center + spread) / denominator,
    )
}

// Usage:
let (lower, upper) = wilson_ci(550, 1000, 1.96);  // 95% CI
// Result: (0.519, 0.580) for 55% win rate
```

### Binomial Test

Test whether win rate is significantly better than 50%:

```rust
use statrs::distribution::{Binomial, Discrete};

/// One-sided binomial test
/// H0: p = 0.50, H1: p > 0.50
pub fn binomial_test(wins: usize, total: usize) -> f64 {
    let binom = Binomial::new(0.5, total as u64).unwrap();

    // P(X >= wins) under null hypothesis
    let p_value: f64 = (wins..=total)
        .map(|k| binom.pmf(k as u64))
        .sum();

    p_value
}

// Usage:
let p = binomial_test(550, 1000);
// Result: p < 0.001 (highly significant)
```

### Information Coefficient

Correlation between signal strength and outcome:

```rust
/// Calculate IC (Spearman correlation between signal and returns)
pub fn information_coefficient(
    signals: &[f64],
    outcomes: &[f64],
) -> f64 {
    // Rank both series
    let signal_ranks = rank(signals);
    let outcome_ranks = rank(outcomes);

    // Pearson correlation of ranks
    pearson_correlation(&signal_ranks, &outcome_ranks)
}

// Interpretation:
// IC > 0.05: Weak but usable
// IC > 0.10: Good signal
// IC > 0.20: Strong signal (rare)
```

### Conditional Probability

P(outcome | signal condition):

```rust
/// Calculate conditional win rate given signal threshold
pub fn conditional_probability(
    signals: &[(f64, bool)],  // (signal_value, is_win)
    threshold: f64,
) -> (f64, usize) {
    let filtered: Vec<_> = signals
        .iter()
        .filter(|(s, _)| *s > threshold)
        .collect();

    let n = filtered.len();
    if n == 0 {
        return (0.0, 0);
    }

    let wins = filtered.iter().filter(|(_, w)| *w).count();
    (wins as f64 / n as f64, n)
}

// Usage:
let (win_rate, n) = conditional_probability(&data, 0.7);
// "When signal > 0.7, win rate is 62% (n=245)"
```

## Sample Size Requirements

### For Edge Detection

To detect a 5% edge (55% vs 50%) with 80% power at α = 0.05:

```
n = (z_α + z_β)² × p(1-p) / e²
n = (1.96 + 0.84)² × 0.5 × 0.5 / 0.05²
n ≈ 784 bets
```

### Practical Thresholds

| Edge Size | Required n (80% power) |
|-----------|----------------------|
| 10% (60% vs 50%) | 196 |
| 5% (55% vs 50%) | 784 |
| 3% (53% vs 50%) | 2,178 |
| 2% (52% vs 50%) | 4,900 |

## Go/No-Go Criteria

### Development Phase (Phase 2)
- [ ] At least 1 signal with p < 0.10
- [ ] Sample size > 50
- [ ] Direction of edge is correct

### Backtest Phase (Phase 3)
- [ ] Composite signal win rate > 53%
- [ ] Binomial p-value < 0.05
- [ ] Positive EV after fees
- [ ] Walk-forward validation passes

### Paper Trading Phase (Phase 5)
- [ ] 200+ paper bets completed
- [ ] Win rate > 52% sustained
- [ ] Risk controls functioning

### Production Phase (Phase 6)
- [ ] All Phase 5 criteria met
- [ ] 100+ live bets with positive returns
- [ ] No edge decay detected

## Validation Report Template

```rust
pub fn generate_validation_report(validation: &SignalValidation) -> String {
    format!(r#"
═══════════════════════════════════════════════════════
SIGNAL VALIDATION REPORT: {}
═══════════════════════════════════════════════════════

SAMPLE STATISTICS
  Total Bets: {}
  Wins: {}
  Win Rate: {:.1}%

STATISTICAL SIGNIFICANCE
  Wilson 95% CI: [{:.1}%, {:.1}%]
  Binomial p-value: {:.4}
  Verdict: {}

PREDICTIVE POWER
  Information Coefficient: {:.3}
  Conditional P(Win | Strong Signal): {:.1}%

GO/NO-GO DECISION
  Production Ready: {}
  Development Use: {}
"#,
        validation.signal_name,
        validation.sample_size,
        validation.wins,
        validation.win_rate * 100.0,
        validation.wilson_ci.0 * 100.0,
        validation.wilson_ci.1 * 100.0,
        validation.binomial_p_value,
        if validation.binomial_p_value < 0.05 { "SIGNIFICANT" } else { "NOT SIGNIFICANT" },
        validation.information_coefficient,
        validation.conditional_probability * 100.0,
        if validation.is_valid_for_production() { "YES ✓" } else { "NO ✗" },
        if validation.is_valid_for_development() { "YES ✓" } else { "NO ✗" },
    )
}
```

## Walk-Forward Validation

Split data into training and test sets:

```rust
pub struct WalkForwardResult {
    pub in_sample_win_rate: f64,
    pub out_of_sample_win_rate: f64,
    pub is_overfit: bool,
}

pub fn walk_forward_validate(
    data: &[TradeResult],
    train_ratio: f64,  // e.g., 0.7
) -> WalkForwardResult {
    let split = (data.len() as f64 * train_ratio) as usize;
    let (train, test) = data.split_at(split);

    let in_sample = calculate_win_rate(train);
    let out_of_sample = calculate_win_rate(test);

    // Check for overfitting: OOS should be within 80% of IS performance
    let is_overfit = out_of_sample < in_sample * 0.8;

    WalkForwardResult {
        in_sample_win_rate: in_sample,
        out_of_sample_win_rate: out_of_sample,
        is_overfit,
    }
}
```

## Common Pitfalls

### 1. Multiple Testing Problem
When testing many signals, some will appear significant by chance.
- Apply Bonferroni correction: α' = α / n
- Or use FDR (False Discovery Rate) control

### 2. Look-Ahead Bias
Signal uses future information not available at decision time.
- All calculations must be point-in-time
- Use proper timestamp filtering

### 3. Survivorship Bias
Only analyzing signals that "worked" in backtests.
- Document all signals tested
- Report both successes and failures

### 4. Small Sample Overconfidence
55/100 = 55% looks great, but CI is [45%, 65%].
- Always report confidence intervals
- Require minimum sample sizes

## CLI Command

```bash
# Validate signals against historical data
cargo run -p algo-trade-cli -- validate-signals \
    --start 2025-01-01 \
    --end 2025-01-28 \
    --signal orderbook_imbalance

# Expected output:
# ═══════════════════════════════════════════════════════
# SIGNAL VALIDATION REPORT: orderbook_imbalance
# ═══════════════════════════════════════════════════════
# ...
```
