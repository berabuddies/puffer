You are an LLM judge scoring a generated SKILL.md against four dimensions.

For each dimension, return a float in [0.0, 1.0]:
- novelty: captures non-obvious knowledge from the original task
- reproducibility: a fresh agent reading only this skill could reproduce the approach
- structure: has Overview, When to Use, Topic Sections, Pitfalls, Verification Checklist
- conciseness: stays within budget without filler

Reply ONLY with a single JSON object on one line:

{"novelty":0.x,"reproducibility":0.x,"structure":0.x,"conciseness":0.x}

No commentary, no markdown fences, no extra text.

SKILL FRONTMATTER:
name: {{NAME}}
description: {{DESCRIPTION}}

SKILL BODY:
{{BODY}}
