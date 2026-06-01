---
name: autodream
description: Consolidate durable project memory from the current conversation and identify traces that may deserve genskill.
allowed-tools:
  - Skill
  - Read
  - Memory
user-invocable: true
disable-model-invocation: false
---

Run AutoDream for the active project. Treat the conversation above as short-term working memory and consolidate only durable project knowledge.

## Phase 1: Orient

- Load the active project memory with the `project-memory` skill before deciding what to change.
- Use only the exact MEMORY.md path exposed by the `project-memory` skill or current memory context. Do not guess alternate memory paths; if a read is denied, retry once with the exact project-memory path before concluding you are blocked.
- Identify existing durable facts, workflow bullets, stale conflicts, and possible polluted entries.
- Treat memory as an explicit patch. For every candidate, choose exactly one action: keep, add, replace, or remove.

## Phase 2: Gather recent signal

- Review the current transcript for durable candidates. Do not wait for the user to say "remember this".
- Prefer candidates confirmed by user correction, tool output, passing verification, stable repository rules, compatibility constraints, repeated workflow shape, or an accepted next-step plan.
- Real traces often contain useful facts mixed with chatter. Extract short durable facts from verified eval results, reusable commands, frozen baselines, known unrelated blockers that affect future test interpretation, stable repo constraints, and user-approved workflows.
- External or benchmark-style noisy trajectories are still valid signal. Do not skip memory merely because a trace is external, failed, unsolved, synthetic-looking, or full of tool noise; if it shows a reusable diagnose-filter-verify method, write one generalized workflow memory entry.
- For external noisy trajectories, generalize away dataset names, task ids, agent/model names, exact file paths, exact commands, temporary paths, secrets, flags, payloads, benchmark artifacts, and incorrect step ids. Preserve the workflow shape: how to inspect, avoid false starts, verify the final state, and decide what not to carry forward.
- For external trajectories, respect the trajectory category when naming workflow memory. Do not default to software-engineering for system-administration, machine-learning, scientific-computing, games, debugging, security, or file-operation traces.
- For benchmark and external task traces, make memory conditional instead of prescriptive. The durable entry should say to read the task prompt, inspect the verifier or expected output contract first, then apply the reusable method only when the current task matches that domain. Do not write memory that could override task-specific instructions.
- When a trace succeeds after applying a compact domain method, preserve the decisive verifier-facing step: exact output schema, required artifact path, permission check, commit-history invariant, exploit/recovery success signal, or focused regression. This is more durable than generic advice like "be systematic".
- When a trace fails or remains unsolved, extract an anti-pattern only if it would prevent repeated exploration. Name the avoided failure mode and the verification signal that should have caught it. Do not promote a failed broad workflow as a positive recipe.
- Preserve verifier-negative lessons when a task looked solved but failed externally. Examples: a service task must leave the service actually running, not only write a setup script; a report task must match the verifier schema and exact CWE labels, not only pass source tests; a certificate/security task must create the requested artifacts even when Bash write-like commands are denied, by switching to allowed file-write tools plus the smallest shell commands that still execute; a polyglot task must satisfy both language entrypoints, not only one implementation.
- For external noisy bug-fix traces, do not stop at a single exact bug fact when the trace also shows a reusable method. Pair the fact with one generalized workflow entry that captures how to localize the failing behavior, inspect the relevant implementation and tests, patch minimally, verify with the focused regression, and discard incidental paths, task ids, and failed probes.
- Keep baseline and blocker memories as facts, not workflows. A baseline, metric checkpoint, known test blocker, or "next step" plan is not GenSkill-worthy by itself.
- Extract reusable workflows when the trace shows a stable method with multiple actions, such as corpus expansion, eval hardening, classifier tuning, prompt tuning, or real-trace rollout.
- For failed or unsolved traces, write memory only when there is a durable recovery lesson or anti-imitation method, such as how to classify false starts, avoid repeating bad tool steps, or verify that a final artifact/result is real.
- When the transcript explicitly labels a verified "Durable workflow:" with four or more reusable actions and a validation signal, write that workflow memory unless the user negates it.
- Suspect first, then narrow: list likely durable candidates mentally, then keep only the ones supported by concrete evidence in the transcript.

## Phase 3: Consolidate memory

- Prefer short imperative entries that preserve exact commands, crate names, file paths, API names, model names, gates, and compatibility constraints needed for future work.
- For durable command-shape memories, preserve reusable flags and knobs that future runs must choose intentionally, including provider/model selectors, runner paths, corpus paths, and explicit concurrency such as `--jobs` or jobs concurrency.
- For workflow memory, store the stable method rather than checkpoint telemetry. Omit version-by-version scores, temporary run ids, job counts, and exploratory run parameters unless they are the durable command or frozen baseline name.
- Start reusable benchmark workflow memories with a scope guard, such as "For log/date tasks..." or "For git-history recovery tasks...". A memory entry without a domain trigger is too broad and should be narrowed before saving.
- Include one "do not" clause when the trace shows wasted exploration, for example do not inspect harness internals before task files, do not rewrite unrelated history, do not trust a recovered artifact before verifier output, or do not reuse a command after the verifier contract contradicts it.
- For E2E task workflows, include a final acceptance guard that is stronger than "tests passed" when the verifier requires artifacts: required files exist, schemas/row order/permissions match exactly, background services are running if required, and any report file uses the exact keys/labels expected by the task.
- If no project-specific fact is durable but a reusable external-trace method is evident, prefer one generalized workflow memory entry over writing nothing. The entry should name the domain, include 4-6 stable actions, include a verification condition, and explicitly exclude the noise class without copying the noisy string.
- For external noisy traces, the fallback memory entry should begin with a clear domain phrase such as "Noisy external software-engineering workflow", "Noisy external file-operation workflow", or "Noisy external security workflow" so later scoring and humans can recognize it as durable workflow memory.
- When both an exact durable bug fact and a reusable external workflow are supported, write both only if the exact fact is likely to recur; otherwise prefer the workflow entry. A workflow-worthy external trace should not end with only a narrow file/API fact.
- If any verified durable candidate exists, write at least one memory entry unless the transcript is purely noise or the user explicitly says not to remember it.
- When replacing, `old_text` must be copied from the existing MEMORY.md entry that was loaded by the project-memory skill. Do not use transcript wording such as "the old note is stale" as `old_text`.
- Replacement is complete only when the stale MEMORY.md entry is gone and the new standalone memory entry captures the verified replacement fact.
- If a replace call fails because no entry matched, immediately retry with the exact stale entry text from MEMORY.md. If there is no stale memory entry, use add instead of replace.
- Never keep both sides of a conflict. Prefer the latest verified workflow over old notes, but preserve unrelated memory entries.
- For reusable multi-step workflows, write one durable workflow memory bullet with the Memory tool before deciding on GenSkill. The bullet must start with the workflow domain and include 4-6 stable actions plus the verification command or success signal when available.
- Workflow memory should help future agents reduce search without increasing tool churn. If the candidate would cause extra generic exploration before acting on a clear task contract, rewrite it to be verifier-first and action-oriented.
- Do not count the workflow as captured if it only appears in the final response. If you will say `AUTODREAM_GENSKILL: yes`, first ensure the workflow bullet is present in MEMORY.md.

## Phase 4: Prune, normalize, and decide GenSkill

- Do not save temporary task progress, one-off local paths, rate limits, transient network failures, shell typos, unverified guesses, abandoned hypotheses, raw selector/API samples from a failed probe, exact run ids unless they are a named frozen baseline, worker names, or details the user said not to remember.
- Do not store meta-instructions such as "do not skill this", "do not remember this", "no GenSkill", or "not a workflow". Use those phrases only to suppress the suggested skill or exclude the named detail from memory.
- Do not invent a durable workflow from a failed tool call, a prompt-tuning attempt, or a rejected Memory edit unless the user explicitly asks to keep the recovered method and a later tool result verifies it.
- When a transcript contains a useful recovery method mixed with local failures, abstract the failure class and omit exact local error strings, bad paths, run ids, worker counts, machine limits, or stale-binary messages.
- GenSkill is separate from memory. Even when `AUTODREAM_GENSKILL: no` is correct, still write durable workflow memory if the generalized method would help future work.
- Decide GenSkill after memory edits. A trace can be GenSkill-worthy even when it is mixed with noise, failed hypotheses, or one-off tool errors; ignore those and judge the reusable workflow that remains.
- For project-native Puffer workflows, use the saved memory entry as the main GenSkill signal. If this pass wrote or preserved a durable workflow entry with a named domain, four or more reusable actions, and a validation or acceptance condition, say `AUTODREAM_GENSKILL: yes` unless a negative rule below applies. This includes eval label-audit workflows, real-trace rollout workflows, classifier/prompt tuning workflows, subagent merge/consolidation workflows, and regression triage workflows.
- Do not require repeated historical examples for project-native workflow memory that already describes a reusable procedure. The fact that the workflow was durable enough to write to MEMORY.md is evidence that it should be considered for GenSkill.
- For external benchmark traces, writing a generalized workflow memory entry is not enough by itself to suggest GenSkill. Suggest GenSkill only when the trace also shows nontrivial transfer value: repeated false starts, incorrect-step filtering, multi-stage recovery, cross-task methodology, a long-tail/unsolved recovery workflow, or a validation pattern that future agents would otherwise rediscover.
- For external benchmark traces, only suggest GenSkill when the saved workflow is both domain-triggered and non-interfering: it tells the future agent when to use it, when not to use it, and to obey the current task prompt and verifier over the skill text.
- Do not suggest GenSkill for lightweight external tasks whose prompt already gives an exact output artifact, schema, date, command, or verifier contract and the saved workflow would mostly restate "read prompt, write artifact, run verifier". Keep those as memory-only guidance.
- Do not suggest GenSkill for catch-all benchmark workflows spanning unrelated domains. A skill-worthy workflow should have one narrow activation domain; otherwise later skill injection can hurt simple tasks by adding irrelevant constraints.
- Treat clean/control-style external traces as negative for GenSkill. If trajectory metadata says `noise_band=clean`, `clean_control=true`, or `selection=full clean control`, say `AUTODREAM_GENSKILL: no` even if you wrote durable workflow memory; this clean-control signal overrides `solved=false`, long-tail wording, and generic reusable workflow shape unless the transcript explicitly labels the workflow as skill-worthy or shows repeated exploration failures.
- For non-clean external traces with `incorrect_error_stage_count>0`, `solved=false`, long-tail step counts, or `noise_band=high|medium|unsolved`, say `AUTODREAM_GENSKILL: yes` when you wrote a durable workflow entry with four or more actions plus a verification signal. Do not require broader transfer evidence beyond that saved workflow for high-noise or unsolved external traces.
- Say `AUTODREAM_GENSKILL: yes` only when a durable workflow memory entry was actually written in this pass or already exists in MEMORY.md, and at least four of these are true: the memory entry describes four or more reusable steps; tool output or verification confirmed the workflow; the workflow applies to future tasks beyond this exact run; the workflow would reduce future search or repeated exploration; it is more than one command or one project convention; the trace shows at least two false starts, incorrect steps, recovery pivots, cross-file/tool phases, or an unsolved/long-tail recovery pattern.
- Say `AUTODREAM_GENSKILL: no` for single commands, one-off conventions, ordinary durable facts, known blocker notes, frozen baseline notes, clean/control-style external tasks, or workflows lacking verification.
- If the marker is `AUTODREAM_GENSKILL: no`, do not say the trace is skill-worthy, do not mention `/genskill`, and do not include any other positive skill suggestion language.
- Before the final response, verify MEMORY.md after edits. If replacement new facts or workflow memory are missing, fix MEMORY.md before answering.
- Keep the final response concise and put `AUTODREAM_GENSKILL: yes` or `AUTODREAM_GENSKILL: no` on its own final line exactly once.
