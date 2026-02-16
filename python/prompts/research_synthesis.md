# Research Synthesis Prompt

You are analyzing market data for **{ticker}** to produce a structured research summary.

## Raw Data

{raw_data}

## Input Context

{input}

## Instructions

Based on the raw data above, produce a comprehensive research summary:

1. **News Summary**: Synthesize the key headlines into 2-3 sentences. Focus on what's material — ignore noise.

2. **Technical Assessment**: Based on price relative to moving averages, RSI, and support/resistance levels, characterize the current technical setup.

3. **Options Flow**: Highlight any unusual activity. Is smart money positioning bullishly or bearishly?

4. **Catalyst Analysis**: Identify the nearest catalyst(s). How clear is the expected impact?

5. **Macro Context**: How does the current macro environment affect this ticker?

6. **IV Assessment**: Is implied volatility cheap or expensive relative to historical? (IV rank: 0-30 = cheap, 30-70 = fair, 70-100 = expensive)

7. **Opportunity Score (1-10)**: Rate the overall quality of the opportunity right now.
   - 1-3: No edge, skip
   - 4-6: Interesting but not actionable yet
   - 7-8: Strong opportunity worth evaluating
   - 9-10: Exceptional setup, evaluate immediately

8. **Key Observations**: List 3-5 bullet points that a thesis analyst should focus on.

Be specific with numbers. Don't hedge everything — take a stance based on the data.
