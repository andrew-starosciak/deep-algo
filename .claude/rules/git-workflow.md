# Git Workflow

## Commit Message Format

```
<type>: <description>

<optional body>
```

Types: feat, fix, refactor, docs, test, chore, perf

Examples:
- `feat: add order book imbalance signal`
- `fix: correct Wilson CI calculation for small samples`
- `refactor: extract signal validation into separate module`
- `test: add property tests for Kelly criterion`

## Branch Strategy

```
main                 # Production-ready code
├── feat/signal-xxx  # New signal development
├── feat/exchange-xx # Exchange integration
├── fix/issue-xxx    # Bug fixes
└── refactor/xxx     # Code improvements
```

## Feature Implementation Workflow

### 1. Research Phase
- Review specs in `specs/` directory
- Understand statistical requirements
- Document hypothesis and validation criteria

### 2. Planning Phase
- Use **planner** agent for complex features
- Break into atomic tasks
- Define Go/No-Go criteria

### 3. Implementation Phase
- Write tests first (TDD)
- Implement minimal passing code
- Validate statistical correctness

### 4. Review Phase
- Use **code-reviewer** agent
- Run `cargo clippy -- -D warnings`
- Verify all tests pass

### 5. Integration Phase
- Run full test suite
- Validate against historical data
- Check for regressions

## Pre-Commit Checklist

```bash
# Run before committing
cargo fmt
cargo clippy -- -D warnings
cargo test
```

## Pull Request Template

```markdown
## Summary
Brief description of changes

## Type
- [ ] New signal
- [ ] Exchange integration
- [ ] Backtest improvement
- [ ] Risk management
- [ ] Bug fix
- [ ] Refactor

## Statistical Validation
- [ ] p-value < 0.05 (if applicable)
- [ ] Sample size > 100 (if applicable)
- [ ] Walk-forward tested (if applicable)

## Test Coverage
- [ ] Unit tests added
- [ ] Integration tests added
- [ ] Property tests added (if applicable)

## Checklist
- [ ] `cargo clippy` passes
- [ ] `cargo fmt` applied
- [ ] All tests pass
- [ ] No `unwrap()` in library code
- [ ] Financial values use `Decimal`
```
