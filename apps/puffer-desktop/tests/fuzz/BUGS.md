# Puffer UI/UX Fuzz Bug List

This file is the main-agent-owned ledger for confirmed or candidate UI/UX fuzz
findings. Subagents should not edit this file directly. They should report a
finding block in their final shard report, then the main agent appends it here
with `puffer-fuzz.mjs bug-list --append`.

## Status Values

- `pending`: accepted as a real product candidate, not fixed yet.
- `fixed`: fixed with regression coverage.
- `duplicate`: same root cause as an existing ledger entry.
- `rejected`: investigated and not a product bug.
- `out-of-scope`: real evidence, but outside the shard or current campaign.

## Ledger

| ID | Status | Severity | Area | Shard | Title | Evidence | Updated |
| --- | --- | --- | --- | --- | --- | --- | --- |

## Details

