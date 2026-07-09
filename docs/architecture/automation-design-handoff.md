# Automation Design Handoff

## Purpose

The desktop Automation tab is a prompt-first place for users to create and
manage simple automations without a canvas. The current implementation is now
connected to daemon Automation contracts for persisted records, catalog loading,
runtime sync, preview runs, activation, deletion, and run history. Svelte state
still owns the in-progress builder/detail editing model and optimistic screen
state.

The design intent is:

- Keep automation creation linear and reviewable.
- Match Puffer's compact desktop visual style.
- Use user-facing automation language.
- Avoid node graphs, infinite canvas controls, and internal status copy.

## Runtime Terminology Boundary

The UI may call selectable capabilities `tools` because that is the user-facing
automation language. Internally, a tool is only an umbrella term:

- `AgentEnv` tools have `runtime_owner: "agentenv"` and compile directly into
  AgentEnv workflow nodes.
- Connector-backed tools have `runtime_owner: "puffer"` and persist as
  `puffer_connector_action` steps. They are Puffer-owned runtime boundaries:
  the daemon executes the connector action through the connector/subscription
  stack, then passes the bridge result into any AgentEnv continuation workflow.
- Connector event triggers are also Puffer-owned. They create automation input
  from a current connector event envelope; AgentEnv sees that event as workflow
  input, not as a connector schema.

Do not treat `puffer_connector_action` as an AgentEnv node type. It is a stable
wire marker for splitting the Automation flow around a Puffer-executed
connector action.

## Current Entry Points

### Sidebar

Automation is available as a sidebar destination in the desktop shell. The
screen title is `Automation`.

### Home Prompt

The home screen starts with `Create an automation` and reuses Puffer's composer
structure:

- Attachment button.
- Model picker.
- Fast toggle.
- Thinking selector.
- Permissions selector.
- Send button.

The placeholder asks users to describe what to automate in natural language.
Submitting the prompt opens the full-page builder and pre-fills configuration
when the prompt matches known patterns. Where the daemon catalog is available,
prefill chooses catalog-backed triggers and tools; otherwise it falls back to
local starter shapes.

### Library Area

Below the prompt, the library is a segmented control:

- `Your automations`
- `Template Library`

`Your automations` starts from the daemon `automation_list` result. It is empty
when no saved daemon records exist. The empty state says
`创建你的第一个automation，处理重复的工作流` and has a `create automation`
action. The toolbar action is `new`.

`Template Library` shows starter cards that open the builder with predefined
name, instructions, and trigger data.

## Current Creation Path

The builder is a full page, not a modal or side panel.

Top bar:

- Breadcrumb back to `Automations`.
- `Create New` label.
- `Cancel`.
- `Save`.

Body sections:

- `Name`
- `Triggers`
- `Instructions`
- `Tools`
- `Run location`

Saving calls the daemon `automation_save` RPC, upserts the returned record into
the local list, and returns to the home screen. Cancel returns to the home
screen without creating a record.

### Natural-Language Prefill

The prompt parser currently recognizes broad keyword groups:

- Pull request prompts become `PR review draft`.
- Calendar, invite, RSVP, or meeting prompts become `Calendar RSVP`.
- Gmail or email prompts become `Email reply draft`.
- Slack, message, or reply prompts become `Reply draft`.
- Daily, weekday, morning, digest, or every prompts become `Morning digest`.

The prefill remains heuristic, but it now prefers daemon catalog entries for
matched triggers and actions. It is still used only to seed a reviewable draft;
the saved record is the daemon `AutomationSpec`, not the prompt parser output.

### Template Starters

Current templates:

- `Review PRs`
- `Reply drafts`
- `Calendar RSVP`
- `Morning digest`

Each template maps to a name, instructions, icon, and initial trigger.

## Current Trigger Model

Triggers are shown as compact sentence rows. The design target for the trigger
picker remains:

- `Every day at` `09:00`
- `Custom schedule` `Cron`
- `PR opened in` `Select repos` `by` `Anyone`
- `Draft opened in` `Select repos`
- `Comment added in` `Select repos`
- `Label changes in` `Select repos`

Added triggers can be edited through the trigger picker or removed from the row.
The trigger picker closes when users click outside the picker.

Implementation progress:

- The primary trigger menu is now loaded from the daemon `automation_catalog`
  result.
- Catalog-backed families currently include `Webhook`, schedule triggers
  (`Every day at`, `Custom schedule`), and connector event triggers for
  connector templates that support workflow triggers.
- Catalog triggers with required inputs render inline configuration fields below
  the row.
- If the catalog cannot be loaded, the UI falls back to the local starter
  triggers listed above.

Current limitations:

- Only one trigger is represented in the UI state at a time, even though
  `AutomationSpec.triggers` supports multiple triggers and the UI says
  `Add Trigger`.
- Existing multi-trigger or rich trigger specs are not fully editable in the UI.
  Rich specs are preserved on save when they are not UI-round-trippable.
- Connector trigger labels and inputs are catalog-backed but still coarse. They
  need richer source app names, event names, app-specific required inputs, and
  configuration state beyond the current generic filter input.
- The trigger search box is visible but does not yet filter the catalog or show
  an empty search result state.

## Current Tool And MCP Model

Tools are selected at the app API capability level. One app can contribute
multiple selectable items, and each capability becomes its own row.

The design target for app groups and capabilities remains:

- GitHub: `Watch Pull Requests`, `Comment on Pull Request`, `Update Commit Status`
- Slack: `Read Slack Channels`, `Send to Slack`, `Reply in Slack Thread`
- Gmail: `Read Gmail Threads`, `Create Gmail Draft`, `Apply Gmail Label`
- Google Calendar: `Read Calendar Events`, `Check Availability`, `Draft RSVP`
- Linear: `Read Linear Issues`, `Create Linear Issue`, `Comment on Linear Issue`
- Notion: `Search Notion`, `Create Notion Page`, `Update Notion Page`

Capabilities with a destination or mode show an inline target chip, such as
`Send to Slack` `to` `#teams`. Target chips cycle through local options.

Selected tools can be edited or removed. The tool picker closes when users click
outside the picker.

`Memories` is always shown as a built-in context tool.

Implementation progress:

- The picker now uses daemon catalog actions instead of hardcoded app mocks.
- The current catalog-backed capability exposed by the daemon is Local Runtime:
  `Local JavaScript Transform`.
- The tool picker search filters app groups and capabilities.
- Catalog-backed rows show connection, permission, and approval-required state
  when the action includes that metadata.

Current limitations:

- Connector actions and MCP tools are not broadly surfaced yet; the daemon
  catalog currently exposes the local transform action and connector-backed
  triggers.
- Required action inputs are represented in the catalog type but do not yet have
  full UI editors.
- Optional targets for some side-effect actions are still local UI choices until
  real connector/MCP target discovery is exposed.

## Current Runtime Model

The builder and detail page include a `Run location` section:

- `Local`
- `AgentEnv Cloud`

New automations default to the configured workflow backend mode. `Configure
Runtime` opens the Automation Runtime settings pane. Saving stores
`run_location` in `AutomationSpec`; active automations compile and deploy through
the selected runtime path.

## Current Detail Page

Clicking a saved automation opens a full-page detail view.

Top bar:

- Breadcrumb back to `Automations`.
- `Test Run`.
- `Save`.
- Overflow menu with `Delete`.

Identity area:

- Editable automation name.
- Active toggle.
- Owner text, currently `You`.

Tabs:

- `Settings`
- `Run History`

### Settings Tab

Settings reuses the builder controls:

- Trigger row and trigger picker.
- Instructions box.
- Tool rows and tool picker.

Changes are local until the user clicks `Save`. Save calls `automation_save`
with the current record revision, updates the returned daemon record in the
local list, and refreshes title, description, status, trigger summary, selected
tools, enabled state, runtime state, and icon.

### Run History Tab

Before a run, the tab shows `No runs yet`.

The tab includes a `Test input` editor. Users can paste a JSON event object or
plain text before running a preview. JSON objects are sent as the preview input;
plain text is wrapped as a text payload.

The design target for a review-oriented run row remains:

- Title: `Test run`
- Status: `Waiting for review`
- Started: `Just now`
- Duration: `-`
- Summary: `Puffer is checking the current configuration.`

Implementation progress:

- Clicking `Test Run` first creates a local `Running` row and switches the
  detail page to `Run History`.
- It saves the current detail edits, calls `automation_sync_preview`, and runs
  `automation_run_preview` with the parsed test input.
- The local running row uses summary
  `Puffer is running the current configuration through daemon preview.`

When the daemon preview completes, the local row is replaced with the preview
result or error. The daemon also appends durable run-history records to
`automation_runs.json`, including status, source event, duration, runtime status,
structured result or error, and preview approval metadata. Opening a detail page
or completing a preview refreshes run history through `automation_run_history`.

After a run, `Result preview` shows the latest run summary, structured output,
or error before the full run-history list.

### Delete

The overflow menu opens a compact action menu. `Delete` calls
`automation_delete`, removes the selected automation from the local list, and
returns to the home screen.

## State Boundaries

Current UI implementation lives in
`apps/puffer-desktop/src/lib/screens/Automation.svelte`.

Important local state:

- `screenMode`: `home`, `new`, or `detail`.
- `savedAutomations`: local saved user automations.
- `selectedAutomationId`: selected detail automation.
- `automationName`, `automationPrompt`, `automationTrigger`, `selectedTools`,
  and `automationEnabled`: active draft/detail edit state.
- `activeAutomationLibraryTab`: home library tab.
- `activeAutomationDetailTab`: detail tab.
- `triggerMenuOpen`, `toolMenuOpen`, and `automationActionMenuOpen`: popup state.
- `automationLoadError`, `automationCatalogError`, `automationSaving`,
  `automationStatusChanging`, and `automationRunning`: daemon interaction state.
- `triggerCatalog` and `commonApps`: daemon catalog state.
- `automationRunLocation`: selected runtime location.

Backend implementation now includes:

- `crates/puffer-automation`: typed `AutomationSpec`, `AutomationRecord`,
  validation, hashing, storage, and compiler support.
- `crates/puffer-cli/src/daemon_automations.rs`: `automation_list`,
  `automation_get`, `automation_save`, `automation_delete`, and
  `automation_catalog`.
- `crates/puffer-cli/src/daemon_automation_runtime.rs`:
  `automation_compile_deploy`, `automation_sync_preview`,
  `automation_run_preview`, `automation_run_history`, runtime compilation,
  local/cloud execution, generated workflow bindings, and run-history storage.

The remaining backend gaps are primarily broader connector/MCP action catalog
coverage, richer trigger/action configuration, approval UI integration, and
production hardening for deployed scheduling across all trigger kinds.

## Interaction Coverage Added

Implemented interactions:

- Open Automation from the sidebar.
- Create from the home prompt.
- Create from `new`.
- Create from a template card.
- Save an automation through daemon RPCs.
- Cancel creation.
- Open saved automation detail.
- Rename automation in detail.
- Edit instructions in detail.
- Toggle active state in detail.
- Save detail edits with revision checks.
- Add, edit, and remove the UI trigger row.
- Render catalog-backed trigger configuration fields.
- Add, edit, remove, and retarget tool rows.
- Select app API capabilities inside the tool picker.
- Search and filter tool picker capabilities.
- Close trigger and tool pickers by clicking outside.
- Switch between `Settings` and `Run History`.
- Sync and execute daemon test-run previews.
- Edit test-run input and preview the latest run result.
- Load durable daemon run history.
- Activate automations through compile/deploy and pause via saved status.
- Open overflow menu and delete a daemon-backed automation.
- Preserve unsupported rich Automation specs while allowing title/instruction
  edits.
- Choose local or AgentEnv Cloud run location and link to runtime settings.
- Keep automation terminology out of visible UI where this screen owns the copy.

## Interactions Not Yet Added

### Creation And Editing

- Full UI editing for multiple triggers in one automation.
- Richer connector-backed trigger options, including precise source app, event
  name, required inputs, and configuration state.
- Real connector actions and MCP tools beyond the current local transform
  catalog action, including capability names, required inputs, optional targets,
  and permission requirements.
- Trigger-specific configuration panels beyond generic catalog inputs, such as
  repo picker, cron editor, contact picker, calendar picker, and label picker.
- Manual editing for trigger target chips.
- Dedicated model picker inside the builder and detail page.
- Runtime health, credential, and workspace detail beyond the current run
  location picker and settings link.
- Dirty state, unsaved-change warning, and save confirmation feedback.
- Keyboard support for closing popups with Escape.
- Keyboard navigation inside trigger and tool menus beyond native button focus.
- Click-outside handling for the overflow action menu.
- Wired search filtering for triggers.
- Empty result state for trigger search.
- More explicit distinction between adding a new tool and editing an existing
  tool when the picker is open.
- Duplicate automation action.
- Archive or pause-from-card action.

### Home And Library

- Search or filter across saved automations and templates.
- Sorting saved automations by recent update, name, status, or source.
- Status chips for saved cards beyond local text.
- Card-level quick actions.
- Template categories.
- Template detail preview before opening the builder.
- Import or paste existing automation configuration.

### Detail Page

- Run history filters.
- Run history detail drawer or timeline.
- Test run input sources, such as selecting a sample event or past message.
- Test run result preview with generated draft, context, and errors beyond the
  current summary and structured result preview.
- More explicit active-toggle success, failure, and pending feedback.
- Delete confirmation.
- Disabled state for destructive or unavailable actions.
- Owner selector or sharing metadata.
- Last saved timestamp.

### Review And Approval

- Review inbox view.
- Pending draft review detail.
- Editable proposed action or draft output.
- Approve, reject, snooze, and edit decision controls.
- Destination preview for outward actions.
- Reason capture for rejected actions.
- A clear audit trail showing who approved what and when.

### Backend And Contracts

- Broader connector-backed tool capability discovery.
- Full permission and credential readiness states across all triggers and tools.
- Field-level validation errors from backend contracts.
- Real execution scheduling hardened across all trigger kinds.
- Workspace or team policy constraints.

## Suggested Next Design Steps

1. Add dirty-state and save feedback to creation and detail pages.
2. Add trigger-specific configuration controls for GitHub repos and schedules.
3. Expand test-run previews with saved sample events and past connector messages.
4. Design the review inbox and approval detail page.
5. Extend backend contracts for richer connector actions, MCP tools, trigger
   configs, field-level validation, and approval metadata.
6. Add delete confirmation and duplicate/archive actions.

## Verification Assets

Current UI coverage is in `apps/puffer-desktop/tests/automation-ui.spec.ts`.

The test suite covers:

- Prompt-first home.
- Empty `Your automations` state.
- Template library.
- Builder layout and controls.
- Trigger and tool picker behavior.
- Capability-level tool selection.
- Saved-card creation.
- Daemon-backed save/update/delete and activation flow.
- Runtime location defaults and runtime settings link.
- Detail page settings.
- Run history empty state, test-run input, daemon preview, and result preview.
- Preservation of unsupported rich Automation specs during UI edits.
- Overflow delete menu visibility.
- Segmented-control background contrast.
