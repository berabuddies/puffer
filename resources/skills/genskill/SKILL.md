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
- the task contract, output artifacts, verifier or success signal, and cases
  where the skill should not override a future task's specific instructions

Output ONLY a SKILL.md document with YAML frontmatter (name, description)
followed by sections: Overview, When to Use, Topic Sections, Common Pitfalls,
Verification Checklist. Stay under 15000 bytes.

Prefer skills that are conditional and verifier-first. A good skill tells the
future agent which task shapes it applies to, what to inspect before acting,
which exact verification signal matters, and how to avoid repeated exploration.
Do not generate broad productivity advice or a recipe that would make a future
agent ignore the current task prompt, required filenames, schemas, or tests.
Prefer a narrow domain skill over a catch-all benchmark skill. If the trace
contains several unrelated task families, split mentally and generate the
single most reusable workflow with the clearest trigger; do not merge logs,
regex, git, certificates, sqlite, service setup, scheduling, and code repair
into one broad skill.
If the available workflow is only a lightweight artifact contract that the next
task prompt will already state exactly, do not turn it into a skill. That
knowledge should remain project memory unless it captures a non-obvious
multi-step method, repeated recovery pattern, or domain-specific verification
procedure that is easy to forget.

When the trace is from an end-to-end benchmark or task runner, preserve
verifier-negative lessons too. A generated skill should prevent "looks done"
failures: writing a setup script when the verifier expects a running service,
passing source tests while writing the wrong report schema/CWE labels, giving up
after a denied shell command when an allowed Write/Edit path can create the
artifact, or satisfying only one side of a polyglot/bidirectional task.
