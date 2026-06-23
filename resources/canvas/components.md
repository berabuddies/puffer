# Canvas component catalog

Machine-readable usage knowledge for the `Canvas` tool's design system. This is
the layer that makes Canvas a *design system*, not a schema: it tells the agent
not just that a component exists, but **when to use it, when not to, and how**.
Compose a tree of these into the `body` of a Canvas spec. Prefer information
density and evidence over decoration.

> Discipline (anti-slop): every claim a node makes — a metric, a finding, a risk
> — should be backed by **evidence** (a file location, a diff, a command output)
> and, where the user could act, an **action**. A node is useful because it lets
> the reader inspect or act, not because it looks nice. Do not emit decorative
> cards/metrics/charts that aren't tied to evidence.

---

## Layout

### section
- purpose: a titled group of related nodes; the main structural unit.
- use_when: you have a labeled part of the page ("Findings", "Coverage").
- props: `title`. children: any nodes.

### grid
- purpose: equal cards laid out responsively.
- use_when: several peer items of similar weight (e.g. metric cards, file cards).
- avoid_when: a single item, or items with a natural order/ranking → use a list or table.
- props: `min` (px, min column width). children: usually `card`.

### columns
- purpose: a few side-by-side blocks (e.g. summary | details).
- props: none. children: 2–3 nodes.

### card
- purpose: a small bordered container for one coherent sub-unit.
- avoid_when: framing a single sentence — just use `text`. (slop signal)
- props: `title?`. children: any.

### divider
- purpose: a thin rule between unrelated blocks. Use sparingly.

## Text
### heading `{level:1|2|3, text}` — a title within content.
### text `{value}` — a paragraph; plain text, preserves newlines.
### badge `{text, tone?}` — a small status label; tone: crit|high|ok|info.

## Data
### metrics
- purpose: an inline summary strip of counts ("3 findings · 1 high · 8 files").
- use_when: a handful of headline numbers at the top of a result.
- avoid_when: a single number (use text), or values needing comparison (use bar).
- props: `items:[{value,label,tone?}]`. tone colors the number (crit|high|ok).

### kv `{rows:[[key,value]]}` — compact key/value facts (metadata, config).
### table `{columns:[..], rows:[[..]]}` — tabular records; the default for lists of structured rows. Use for precise values and ranking.

## Viz
### bar
- purpose: compare a small set of categories by magnitude.
- use_when: "energy transition speed by region", relative scores.
- avoid_when: time series (not supported), or part-of-whole percentages.
- props: `items:[{label,value,max?,tone?}]`.

### heatmap
- purpose: intensity across two axes (region × risk dimension).
- use_when: a small matrix where relative intensity matters more than exact value.
- props: `columns:[..]`, `rows:[{label, cells:[0..4]}]` (0 none → 4 critical).

## Input
Interactive nodes collect typed user choices that the agent can later read with
`CanvasState`. Every interactive node MUST have a stable `id`; that id becomes
the key in `CanvasState.values`.

### toggle `{id,label,value?,help?}`
- purpose: a binary yes/no or enabled/disabled choice.
- use_when: the user should include/exclude one behavior.

### singleSelect `{id,label,options:[{id,label}],value?,help?}`
- purpose: one choice from a short option set.
- use_when: the options are mutually exclusive.

### multiSelect `{id,label,options:[{id,label}],value?:string[],help?}`
- purpose: several independent choices from a short option set.
- use_when: the user may select more than one area, file, filter, or action.

### barSelect `{id,label,options:[{id,label}],value?,help?}`
- purpose: one choice that benefits from being displayed near comparative bars.
- use_when: the user is choosing a ranked category rather than entering data.

### slider `{id,label,min?,max?,step?,value?,help?}`
- purpose: a numeric preference or threshold.
- use_when: the exact number can be approximate and adjusted visually.

### textInput `{id,label,placeholder?,value?,help?}`
- purpose: a short free-form value.
- avoid_when: collecting secrets or long text; use dedicated secret/user input
  tools instead.

## Evidence + rich (carry the proof)
### finding
- purpose: one issue/observation that the reader may need to act on. The core
  unit of a code review / audit.
- MUST carry: `severity` (critical|high|medium|low|info) and `title`.
- SHOULD carry evidence: `locations:[{path,line}]` and/or `evidence` (diff/code text).
- SHOULD carry `actions` when the user can do something (fix/test/explain).
- props: `{severity,title,status?,locations?,body?,evidence?,actions?:[{id,label,intent?}]}`
  intent: fix|test|explain.
- avoid_when: a neutral fact with no severity → use text/kv. Don't inflate notes
  into findings.

### diff `{text}` / code `{text}` — monospace block; `+`/`-` lines are tinted as a diff. Use to show the actual change/evidence, not to paraphrase it.
### callout `{tone, title, body}` — ONE highlighted takeaway (the single most important thing). tone: crit|high|ok|info. Use at most once near the top. Overuse is a slop signal.

---

## Composition notes
- Lead with a `metrics` strip or a single `callout`, not both.
- Rank findings by severity; the highest-impact item first.
- Keep evidence next to the claim (diff inside the finding, not in a separate box).
- Pick the component that fits the data: ranking → table/bar, part-of-whole →
  (not bar — say so in text), matrix → heatmap, issue-to-act-on → finding.
