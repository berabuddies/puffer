# Puffer UI/UX Fuzz Candidate Ledger

This file is the main-agent-owned ledger for UI/UX fuzz candidates that cite
real replay evidence but do not yet satisfy the deterministic predicate gate.
Subagents should not edit this file directly.

## Status Values

- `candidate`: real cited evidence, awaiting reviewer or human decision.
- `soft-bug`: reviewer accepted the candidate as likely product-relevant.
- `dismissed`: reviewer or human dismissed the candidate.
- `human-queue`: requires manual review.

## Ledger

| ID | Status | Area | Shard | Title | Evidence | Updated |
| --- | --- | --- | --- | --- | --- | --- |

## Details

