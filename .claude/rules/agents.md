# Agent Orchestration

## Available Agents

| Agent | Purpose | When to Use |
|-------|---------|-------------|
| planner | Implementation planning | Complex features, multi-phase work |
| architect | System design | Architectural decisions, new crates |
| tdd-guide | Test-driven development | New signals, risk management |
| code-reviewer | Code review | After writing code |
| security-reviewer | Security analysis | Wallet integration, API keys |
| build-error-resolver | Fix build errors | When cargo build fails |
| go-reviewer | Go code review | N/A (Rust project) |
| database-reviewer | Database review | Schema changes, query optimization |

## Statistical Trading-Specific Usage

### Signal Development
1. **Research phase** - Use planner to outline signal hypothesis
2. **Implementation** - Use tdd-guide to write validation tests first
3. **Review** - Use code-reviewer after implementation

### Backtest Development
1. **Metrics design** - Use architect for statistical framework
2. **Implementation** - Use tdd-guide for metric calculations
3. **Validation** - Manual review of statistical correctness

### Exchange Integration
1. **API research** - Use planner to outline endpoints
2. **Security** - Use security-reviewer for auth handling
3. **Implementation** - Use tdd-guide for API client

## Immediate Agent Usage

No user prompt needed:
1. Complex feature requests → **planner** agent
2. Code just written/modified → **code-reviewer** agent
3. New signal implementation → **tdd-guide** agent
4. Schema changes → **database-reviewer** agent
5. Wallet/API key handling → **security-reviewer** agent

## Parallel Task Execution

ALWAYS use parallel Task execution for independent operations:

```markdown
# GOOD: Parallel execution
Launch 3 agents in parallel:
1. Agent 1: Review signal generator implementation
2. Agent 2: Review database schema changes
3. Agent 3: Security review of API authentication

# BAD: Sequential when unnecessary
```

## Project-Specific Considerations

### Financial Code Review
- Verify `Decimal` usage for all money values
- Check for proper error handling (no unwrap on financial ops)
- Validate fee calculations

### Statistical Code Review
- Verify hypothesis test implementations
- Check confidence interval calculations
- Validate sample size requirements

### Exchange Integration Review
- Rate limiting implementation
- Authentication security
- Error recovery and reconnection
