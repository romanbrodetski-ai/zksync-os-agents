## Summary

Updates `zksync-os-server` from `4f51503f353d89f87046cb0be21c005cd6e0a606` to `ba5821a6bd47abc396c30196f3af475d44fd37f3` and refreshes the restart agent coverage for the sequencer split, startup replay handoff, loopback canonization wiring, and the batch persistence timing fix relevant to post-restart settlement.

## Severity Of Issues Found

none

## Scope Of Impact On This Agent's Feature Area

major

## Issues / Comments Found In The Diff

- None in the reviewed transaction-inclusion-across-restart path.

## Reviewer Notes

- The important behavioral change in this range is that block execution can run ahead of persistence via `OverlayBuffer`, while restart recovery still depends on durable replay/state/repository writes.
- Existing tests already covered receipt survival and post-restart settlement; this PR adds explicit coverage that the main node resumes including fresh transactions after replay on restart.
- The knowledge base was added because this agent repo did not yet carry the compact feature model / test-plan / final-report structure expected for ongoing maintenance.
