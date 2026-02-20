# Thesis Scoring Prompt

You are evaluating a trade thesis for options trading. Score it rigorously.

## Research Summary

{input}

{historical_context}

{system_feedback}

## Scoring Framework

### Pre-Scoring: Structured Debate

Before scoring, explicitly consider both sides:

**Bull Case**: What is the strongest argument FOR this trade? What information
advantage exists? What catalyst could drive outsized returns?

**Bear Case**: What is the strongest argument AGAINST? What could go wrong?
What is the market already pricing in? What risks are being underweighted?

Only after considering both sides should you score each dimension.

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

## Expected Move & Timeline (REQUIRED)

Provide your estimates for programmatic contract selection:

1. **expected_move_pct**: How much the stock should move if thesis plays out (in %).
   Be realistic: most catalyst moves are 5-15%. Earnings surprises 10-30%.
2. **catalyst_timeline_days**: Days until the catalyst fires. Be specific.

Do NOT recommend a specific options contract — set `recommended_contract` to null.
The system selects contracts automatically using real IB market data (strikes, expirations, bid/ask).

## Output

Produce a complete Thesis with scores, evidence, risks, expected_move_pct, and catalyst_timeline_days.
Set recommended_contract to null — the system handles contract selection.
Be decisive — don't give every dimension a 5. Take a stance.
