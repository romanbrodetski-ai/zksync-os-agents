## Happy Path

- `included_tx_survives_main_node_restart`
  Protects durable receipt and balance continuity for an already included tx.
  Fails if replay/state/repository persistence becomes incomplete at restart.

- `restart_after_inclusion_before_finality_still_settles`
  Protects that an included block continues through safe/finalized after restart.
  Fails if restart breaks L1-driven post-inclusion progression.

- `main_node_can_include_new_tx_after_restart_replay`
  Protects that replay hands control back to normal production after restart.
  Fails if startup gets stuck in replay-only mode or post-restart sequencing stalls.

## Boundary / Ordering

- The new test also checks that the post-restart tx lands in a different block than the pre-restart tx.
  This catches accidental reuse of stale receipt data instead of fresh production.

## Gaps

- No direct crash-in-the-middle-of-persistence test; the harness only exercises clean process kill and restart.
- No direct WAL corruption / partial-write coverage.
- No external-node restart coverage in this agent.
