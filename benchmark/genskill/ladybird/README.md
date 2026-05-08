# /genskill Ladybird PR Replay Benchmark

This is the implementation of Plan 3 from
`docs/superpowers/specs/2026-05-07-genskill-design.md`. The full design is in
`docs/superpowers/specs/2026-05-07-genskill-eval-ladybird.md`.

## Quick start

```bash
# Validate corpus on disk
cargo run -p puffer-genskill-eval -- validate

# Run a single replay
cargo run -p puffer-genskill-eval -- replay pr-12345 gepa

# Aggregate finished replays into a report
cargo run -p puffer-genskill-eval -- aggregate 2026-05-20
```

Cost per full run (30 replays): about $35-70 with Sonnet.
