# L1-Settle Agent

This agent owns the "Settling Batches on L1" feature area. It reviews PRs that touch the L1 settling pipeline, keeps the knowledge base current, and adds or updates tests when the code changes.

See the root `AGENTS.md` for the general review process, tone, and style.

---

## Activation

Invoke this agent when a PR touches any file from the in-scope list in `knowledge/overview.md`:

- `lib/l1_sender/src/commands/` (commit, prove, execute)
- `lib/l1_sender/src/lib.rs`
- `lib/contract_interface/src/calldata.rs`
- `lib/contract_interface/src/models.rs`
- `node/bin/src/prover_api/gapless_committer.rs`
- `node/bin/src/priority_tree_steps/priority_tree_pipeline_step.rs`
- `lib/l1_watcher/src/`
- `node/bin/src/lib.rs` (L1 settling wiring sections)

Also invoke when a PR explicitly touches `l1-settle/tests/`.

---

## Knowledge

Read `knowledge/overview.md` (invariants, edge cases) and `knowledge/final-report.md` (mutation coverage). High-severity issues include: incorrect on-chain state, silent data corruption, transaction reverts, or security regressions (ordering, calldata encoding, 2FA, SNARK public input, StoredBatchInfo hash, L1 tx lifecycle).

Also check `knowledge/open-questions.md` — if the PR resolves an open question, remove or update the entry.

---

## Setup note

After bumping the submodule, copy forge-compiled contract artifacts (gitignored, absent in fresh checkouts):

```sh
cp -al ../zksync-os-server/integration-tests/test-contracts/out zksync-os-server/integration-tests/test-contracts/out
```

---

## Tests

```sh
cargo nextest run -p zksync_os_l1_settle_tests --test unit_calldata \
  --test unit_stored_batch_info --test unit_snark_public_input \
  --test unit_2fa --test unit_execute
```
