# Thesis Scoring Prompt

You are evaluating a trade thesis for options trading. Score it rigorously.

## Research Summary

{input}

## Scoring Framework

Score each dimension 1-10:

### Information Edge
- 1-3: You're behind the market — everyone already knows this
- 4-5: You're seeing the same thing as everyone else
- 6-7: You've identified a nuance most haven't focused on
- 8-10: You've connected dots from multiple sources that aren't reflected in price

### Volatility Pricing
- Based on IV rank (from research summary)
- IV rank < 20: Score 9-10 (vol is very cheap, great for buying options)
- IV rank 20-35: Score 7-8 (vol is cheap)
- IV rank 35-50: Score 5-6 (fair value)
- IV rank 50-70: Score 3-4 (expensive, consider spreads)
- IV rank > 70: Score 1-2 (very expensive, avoid buying naked options)

### Technical Alignment
- Does the chart support the thesis direction?
- Price above rising MAs + breakout = high score for bullish thesis
- Price below falling MAs + breakdown = high score for bearish thesis
- Contradiction between thesis and technicals = low score regardless

### Catalyst Clarity
- 9-10: Specific date, specific expected outcome (earnings with clear bull/bear case)
- 7-8: Known event within 2 weeks, reasonable expected impact
- 5-6: General theme, timeline somewhat clear
- 3-4: Vague sector thesis, no specific timeline
- 1-2: No identifiable catalyst

### Risk/Reward Ratio
- Calculate expected gain if thesis plays out vs max loss
- Minimum acceptable: 2:1
- Prefer 3:1+

## Contract Selection (if overall >= 7.0)

If the thesis scores 7.0+, recommend a specific contract:

1. **Expiry**: At least 2x the catalyst timeline. Catalyst in 2 weeks → minimum 4-week expiry.
2. **Strike**: Slightly OTM for leverage, or ATM for higher probability.
3. **Strategy**: If IV rank > 50, consider debit spread. If IV rank < 50, naked option.
4. **Liquidity**: Bid-ask < 10% of mid price. OI > 500.

## Output

Produce a complete Thesis with scores, evidence, risks, and contract recommendation.
Be decisive — don't give every dimension a 5. Take a stance.
