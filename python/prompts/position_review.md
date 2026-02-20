# Position Review Prompt

You are reviewing an open options position as part of pre-market research.

## Current Position

{positions}

## Original Thesis

{original_thesis}

## Previous Reviews

{previous_reviews}

## Recent News & Price Action

{input}

{review_patterns}

## Review Framework

Evaluate this position against the fresh research data above:

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

Provide:
- Whether the thesis is still valid (true/false)
- Your recommended action (hold/add/reduce/close/roll)
- Clear reasoning for your recommendation
- Any urgent concerns
