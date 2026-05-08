You are generating one of N candidate SKILL.md files from an execution trace.

The trace is provided as YAML at the end of this prompt. Treat it as evidence
of a non-trivial task that a future agent might want to redo. Your job is to
distill it into a reusable skill document.

Output ONLY a SKILL.md document with this structure:

```
---
name: <lowercase-hyphen-name, up to 64 chars>
description: <one-line use-case trigger, up to 1024 chars>
---

# <Title>

## Overview
<2-4 sentences on what this skill does>

## When to Use
<bullet list of triggers>

## <Topic Sections>
<the substantive content: what was learned, how to do it>

## Common Pitfalls
<bullet list of edge cases>

## Verification Checklist
<bullet list to confirm the skill worked>
```

Constraints:
- Stay under 15000 bytes total
- ASCII only
- No commentary outside the SKILL.md document

EXECUTION TRACE:
{{TRACE_YAML}}
