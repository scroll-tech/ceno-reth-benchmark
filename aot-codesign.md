# AOT Preflight–Witness Co-Design

## Decision

Accepted for the production `CENO_GPU_WITGEN=0` path on 2026-07-28. The
five-run warm combined median is `19.779601292 s`, versus `24.319528475 s` for
the control, an `18.668%` reduction. The acceptance ceiling was
`20.671599204 s` (15% reduction).

The accepted Ceno checkpoints are:

- `c6b91764 feat(aot): annotate cross-shard accesses with a fixed tape`
- `77019d66 perf(aot): keep dense non-MMIO memory accesses native`
- `aead86ed perf(aot): aggregate register accesses at block boundaries`

The direct CPU witness consumer is checkpointed in Ceno-GPU as
`8aba3c65 feat(witgen): consume future-access annotations directly`.
`CENO_GPU_WITGEN=1` is deliberately outside this result.

## Run configuration

- Block `25580200`, cached mainnet input selected by `--chain-id 1`.
- Release features `jemalloc,gpu,aot`; every run logged `CUDA Backend Enabled`.
- CPU affinity: logical CPU 0.
- `CENO_GPU_WITGEN=0`.
- Cell budget `268435456`; cycle budget `536870912`.
- Candidate artifact: ABI 6, `173 MiB`, cache hit in every measured warm run.
- Combined metric: AOT preflight plus the complete shard-0
  `app_prove.generate_witness` span. That span includes FullTracer replay and
  witness assignment; `position_next_shard` is not added again.

## Five-run warm gate

All values are seconds.

| Run | Control preflight | Control witness | Control combined | ABI-6 preflight | ABI-6 witness | ABI-6 combined |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 21.730118086 | 2.30 | 24.030118086 | 17.699697997 | 2.09 | 19.789697997 |
| 2 | 21.979528475 | 2.34 | 24.319528475 | 17.659601292 | 2.12 | 19.779601292 |
| 3 | 22.016071391 | 2.31 | 24.326071391 | 17.684904075 | 2.15 | 19.834904075 |
| 4 | 21.932578795 | 2.29 | 24.222578795 | 17.630087528 | 2.10 | 19.730087528 |
| 5 | 22.692353577 | 2.28 | 24.972353577 | 17.571442720 | 2.11 | 19.681442720 |
| Median | 21.979528475 | 2.30 | 24.319528475 | 17.659601292 | 2.11 | 19.779601292 |

The candidate/control ratio is `0.813324`, an `18.668%` improvement. The
candidate clears the required ceiling by `0.891997912 s`.

## Correctness and capacity

The ABI-6 cold training/proof run and all five warm runs preserved:

- block hash `0x34439c597563024690ce3c91a082c34507569c7e18cc4d1b3b68550b791a2773`;
- `994,896,527` guest instructions and `3,979,586,112` internal cycles;
- final shard-0 proof verification;
- `StepRecord` size of 136 bytes;
- fixed tape capacity `28,492,288`, usage `25,334,834`, and zero overflow;
- zero normal-path Rust access callbacks;
- fallback `2,036,036` (`0.20%`): `550,681` dynamic-PC misses, zero memory
  guards, and the remainder ecalls;
- 657 greedily planned shards, with shard boundaries allowed to differ from
  interpreter planning.

Tape finalization was `0.978–1.022 s`, of which radix sorting was
`0.809–0.847 s`. Shard-0 witness generation was `1.98–2.15 s`; its
`position_next_shard` portion was `111 ms` in the correctness run. Focused AOT
preflight differential tests and the repeated-address tape ordering test pass.

## Profiling and iteration history

Time-filtered `perf` sampling of the preflight interval showed the remaining
cost dominated by generated guest block bodies. The largest named Rust costs
were secp256k1 modular inversion (`6.4%`) and field multiplication (`4.5%`);
`ShardPlanBuilder::observe` was `1.1%` and `aot_exec_one` was `0.6%`. Within a
representative hot generated block, samples split approximately 53/69 in the
guest body, 9/69 in guards, and 7/69 in block-cost accounting. A comparable
multi-event `perf stat` distribution was not retained because multiplexing
distorted the run, so cache/TLB/branch counter values are intentionally not
reported as if they were gate-quality measurements.

Additional iterations were kept long enough to profile and prove, then the
tracked source was returned to the accepted ABI-6 checkpoint:

| ABI | Experiment | Warm/representative preflight | Result |
| ---: | --- | ---: | --- |
| 7 | Collapse every memory block by dynamic address | `17.899 s` | Slower; a 545-access block caused quadratic comparisons and a 203 MiB artifact. |
| 8 | Collapse only small blocks with repeated static address expressions | `17.754 s` | Correct but neutral/slower; scratch capacity 8 and 173 MiB artifact. |
| 9 | Reuse prevalidated block-entry addresses to skip instruction guards | `18.574 s` | Correct but slower; scratch traffic and entry bound bookkeeping dominated. |

These are rejected follow-on experiments, not a rollback of the accepted
co-design. Their logs are retained under
`.codex-results/aot-codesign-20260728/iterate4` through `iterate6`.

## Next bottleneck

The next optimization should target generated guest-body work and the exposed
cryptographic syscall cost, not witness future-access lookup. A memory-block
aggregation design should avoid quadratic generated comparisons and should be
evaluated first as a fixed-size native hash/sort scratch scheme. Fusing direct
FullTracer record emission remains the architectural route to remove the
remaining replay span.

## Outstanding promotion check

The circuit-aware WITGEN=0 sanity for block `23587691` could not start. Its
cached input (`block_data/input/1/23587691.bin`, dated 2025-12-11) is
incompatible with the current bincode schema and fails decoding with
`unexpected string`. No RPC environment is configured to refresh the cache.
The stale cache was left untouched; older proof artifacts were not presented
as validation of this checkout. After refreshing that cached input, rerun
execute sanity with `CENO_MAX_CELL_PER_SHARD=2684354560`, require the calibrated
two-shard shape, then prove shard 0.

## Artifacts

- Control logs: `.codex-results/aot-codesign-20260728/control/`.
- Preliminary tape-only logs: `.codex-results/aot-codesign-20260728/candidate/`.
- Accepted ABI-6 cold proof: `iterate3/train.log`.
- Accepted ABI-6 five-run gate: `iterate3/warm/run-1.log` through `run-5.log`.
- Profiling data: `iterate1/preflight-perf.data` and
  `iterate1/preflight-perf.log`.
- Rejected follow-on proof logs: `iterate4/train.log`, `iterate5/train.log`,
  and `iterate6/train.log`.
