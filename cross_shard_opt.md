# Cross-Shard Materialization Optimization

## Goal

Reduce `shard-id 0 current_shard_external_write` by avoiding data that is allocated or materialized in shard 0 but only used in later shards.

The useful direction is not heap recycling. The useful direction is to keep untrusted witness data in hints/raw form and materialize only the small piece needed immediately before actual use. Any streamed hint data remains untrusted and must be verified against the same trie/root checks as the non-streaming path.

## Change Summary

This change makes the state witness path more just-in-time:

- The state trie and storage trie byte payloads are read as raw hint slices instead of eagerly copied into heap-owned `Bytes`.
- The full parent state trie decode is delayed until the final bundle-state update needs it.
- Account reads during EVM execution are streamed as `AccountInput` witness items in witness-access order.
- The guest caches only the small `Option<TrieAccount>` values that are actually used.
- Before applying bundle updates, the guest decodes the parent state trie and validates every streamed account against it.

This preserves the important trust boundary: hint data is not trusted just because it is streamed. It is checked against the decoded parent state trie before the final state-root transition is accepted.

## Validation Result

Validated on block `23587691`, shard `0`, with `CENO_DEBUG_SHARD_RAM=1` and GPU sanity e2e.

Baseline before account streaming:

- Log: `sanity_23587691_shard0_witgen1_debug_shard_ram_final_20260430_165048.log`
- Instructions: `24127821`
- Cycles: `96511288`
- Shard boundaries: `[4, 54257372, 96511288]`
- Total ShardRAM rows: `141272`
- `current_shard_external_write`: `129862`
- `current_external_write_heap`: `62893`
- `current_external_write_hints`: `61945`

After account streaming and validation:

- Log: `sanity_23587691_shard0_witgen1_debug_shard_ram_account_chunks_checked_20260430_170606.log`
- Instructions: `24626808`
- Cycles: `98507236`
- Shard boundaries: `[4, 60696700, 98507236]`
- Total ShardRAM rows: `171677`
- `current_shard_external_write`: `53991`
- `current_external_write_heap`: `25404`
- `current_external_write_hints`: `25269`

Net effect:

- `current_shard_external_write`: `129862 -> 53991`, down `75871` rows, about `58.4%`.
- Heap external write: `62893 -> 25404`, down `37489` rows, about `59.6%`.
- Hints external write: `61945 -> 25269`, down `36676` rows, about `59.2%`.

The total ShardRAM row count increases because the streaming path performs more just-in-time hint reads and validation work. That is acceptable for this experiment because the target bottleneck is cross-shard external write from shard 0, not total row count.

## Correctness Requirements

The streaming refactor must not weaken validation:

- Hint data is untrusted.
- Streamed account witness items must be validated against the parent state trie before state update.
- The decoded parent state trie root must match the ancestor header state root.
- The final post-execution state root must still be derived by applying bundle updates to the verified trie state.
- Future storage streaming must keep equivalent validation, either by checking streamed slots against decoded storage tries before update or by using a sound per-storage transition proof.

## Next Direction

The remaining promising optimization is storage-slot streaming:

- Stream per-slot values/proofs immediately before `storage_ref` instead of materializing whole storage tries early.
- Cache only slot values needed by execution.
- Validate streamed storage values against the corresponding storage trie before final update.
- Preserve all parent root and post-state-root checks.

This follows the same principle that produced the current drop: avoid early shard-0 materialization of data that survives across shard boundaries.
