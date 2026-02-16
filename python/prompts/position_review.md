# Position Review Prompt

You are reviewing open options positions in a midday/post-market check.

## Current Positions

{positions}

## Recent News & Price Action

{input}

## Review Framework

For each open position, evaluate:

1. **Thesis Status**: Is the original thesis still intact? Has any new information changed it?
   - New positive information → thesis strengthened
   - Neutral / no change → thesis unchanged
   - Negative news / thesis-breaking event → thesis weakened/invalidated

2. **P&L Assessment**: Current unrealized P&L. How much is from delta vs theta decay?

3. **Recommended Action**:
   - **Hold**: Thesis intact, not at any exit trigger
   - **Add**: Thesis strengthened AND we have room in allocation
   - **Reduce**: Thesis weakened but not invalidated, or approaching targets
   - **Close**: Thesis invalidated, or risk/reward no longer favorable
   - **Roll**: Thesis intact but timing was off — extend expiry

4. **Urgency**: Is this action needed today, or can it wait?

## Summary

After reviewing all positions, provide:
- Overall portfolio assessment
- Any urgent actions needed
- Any emerging cross-position themes or concerns
