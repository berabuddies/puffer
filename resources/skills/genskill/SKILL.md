---
name: genskill
description: Generate a reusable skill from the current conversation history using a GEPA-style multi-candidate loop with LLM-as-judge Pareto selection.
disable-model-invocation: false
---

Generate a reusable SKILL.md from the conversation transcript so far.

Treat the transcript as evidence of a non-trivial task. Extract:
- novel knowledge that would surprise a fresh agent
- edge cases hit during the task
- domain knowledge reconstructed during the work
- the approach in a form a fresh agent could reproduce

Output ONLY a SKILL.md document with YAML frontmatter (name, description)
followed by sections: Overview, When to Use, Topic Sections, Common Pitfalls,
Verification Checklist. Stay under 15000 bytes.
