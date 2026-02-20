# Thesis Critique Prompt

You are stress-testing the following trade thesis as a devil's advocate.
Your job is to find weaknesses, not to be agreeable.

## Thesis Under Review

{input}

{system_feedback}

## Your Task

### 1. Build the Counter-Case

Construct the strongest possible argument AGAINST this trade:
- What is the market already pricing in?
- What catalysts could fail or be delayed?
- What macro/sector risks are being ignored?
- Is the IV rank being read correctly for this strategy?

### 2. Identify Red Flags and Blind Spots

Flag specific concerns:
- Is the information edge real, or is this well-known consensus?
- Is the catalyst timeline realistic?
- Are the risks listed actually the biggest risks, or are important ones missing?
- Does the expected move seem reasonable for this type of catalyst?

### 3. Adjust Scores

Based on your critique, output the **complete Thesis** with adjusted scores:
- If a dimension was over-scored, lower it with justification
- If risks were missing, add them to the `risks` list
- Append "[Critic] <your key insight>" to the end of `thesis_text`
- Recalculate overall score — if it drops below 6.0, the trade will be aborted

### 4. Decision

- If the thesis is fundamentally sound despite your critique, keep scores near original
- If there are serious flaws, lower scores aggressively — the system will abort below 6.0
- Be honest: a trade that survives tough scrutiny is stronger for it

## Output

Return the complete Thesis object with your adjustments. Same schema, adjusted values.
Set `recommended_contract` to null (unchanged from input).
