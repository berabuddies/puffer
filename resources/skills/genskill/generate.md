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
- Make the skill conditional: include clear triggers in When to Use and clear
  non-goals or "do not use when" cases when the trace contains failure modes.
- Prefer a narrow domain skill over a catch-all benchmark skill. If the trace
  mixes unrelated task families, choose the single highest-signal reusable
  workflow and exclude the others in When to Use / non-goals.
- If the trace only teaches a lightweight artifact contract that future task
  prompts already state exactly, output a very narrow skill only when there is
  a non-obvious recovery or verification method; otherwise the caller should
  keep it as memory rather than generating a skill.
- Make the skill verifier-first: preserve required output files, schemas,
  commands, tests, success signals, and minimal checks from the trace.
- Preserve task-specific constraints that prevent regressions, but generalize
  temporary paths, run ids, model names, and incidental benchmark noise.
- Do not write generic advice that could override a future task prompt. The
  skill must instruct the future agent to obey current task files and verifier
  expectations over the skill when they differ.
- Preserve "looks done but verifier failed" lessons when present: service tasks
  may require a running service rather than a script, report tasks may require
  exact JSON keys and CWE labels, denied shell commands may require switching to
  Write/Edit plus smaller executable commands, and polyglot tasks may require
  both language entrypoints to work.

EXECUTION TRACE:
{{TRACE_YAML}}
