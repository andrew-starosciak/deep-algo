# Hooks System

## Recommended Pre-Commit Hooks

### Rust Quality
```bash
# Pre-commit hook
cargo fmt --check
cargo clippy -- -D warnings
cargo test --lib
```

### Financial Code Validation
- Check for `f64` usage in financial calculations
- Verify `Decimal` usage for prices, quantities, P&L

## Post-Implementation Hooks

### After Signal Implementation
1. Run statistical validation
2. Check p-value < 0.10 (development threshold)
3. Verify sample size sufficient

### After Exchange Integration
1. Security review for credentials
2. Rate limiting verification
3. Error recovery testing

## TodoWrite Best Practices

Use TodoWrite tool to:
- Track progress on multi-phase implementations
- Document Go/No-Go decision points
- Track statistical validation status

Example:
```
Phase 1: Data Infrastructure
- [x] Order book collection
- [x] Funding rate collection
- [ ] Liquidation collection
- [ ] News collection
Go/No-Go: Data flowing for >24h

Phase 2: Signal Development
- [ ] Order book imbalance (p < 0.10)
- [ ] Funding reversal (p < 0.10)
Go/No-Go: At least 1 signal validates
```
