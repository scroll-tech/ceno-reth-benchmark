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

This 2026-07-28 priority is superseded by the matched 2026-07-31 result below.
Frontend/iTLB pressure was real, but reducing it did not reduce total cycles
once cold native work became Rust recovery. The retained secp256k1 double fix
still removes the largest avoidable callback cost. Do not resume the rejected
quadratic memory-collapse or prevalidated-address scratch designs. Fusing
direct FullTracer record emission remains the architectural route to remove the
remaining replay span.

## 2026-07-28 CPU profiling and secp256k1 double

The profiling-only Ceno checkpoint is `fd36c95f` and the retained crypto
checkpoint is `f996f82b`. `CENO_CPU_PROFILE_PHASES=1` provides stable begin/end
symbols and optional `perf stat --control=fifo:` signaling for AOT execution,
FullTracer replay, and witness assignment. Profile artifacts use a separate
`-cpu-profile` cache key; accepted ABI-6 artifacts are unchanged. Generated
images now expose dispatch, guard, accounting, guest, memory, commit, fallback,
and Rust callback symbols.

One cold correctness run measured AOT execution at `17.516455692 s`, shard-0
FullTracer replay at `119.53972 ms`, and witness assignment at
`1.950901786 s`. It preserved the canonical hash, instruction/cycle totals,
fixed tape capacity/usage, zero overflow/callbacks, and verified shard 0.

Non-multiplexed, CPU-0-pinned AOT-only counters reported:

| Distribution | Result |
| --- | ---: |
| Host cycles / instructions | `78,307,657,755` / `155,809,004,414` |
| Global IPC | `1.99` |
| Frontend-idle cycles | `29.42%` |
| Branch misses | `4.60%` of `18,685,182,298` branches |
| Generic cache misses | `36.53%` of `7,910,550,940` references |
| dTLB load misses | `2.40%` |
| iTLB load misses | `42.01%` |

Grouped cycle/instruction sampling attributed `75.09%` of cycles and `56.94%`
of instructions to the generated image (recorded as the only unresolved user
DSO because recording began disabled), implying approximately `1.51` local IPC.
The largest named callback functions were secp256k1 modular inversion
(`7.41%` cycles, approximately `3.66` local IPC), field multiplication (`5.49%`,
approximately `3.67`), and field squaring (`1.69%`, approximately `3.71`). Thus
the crypto routines were hot but not the low-IPC source. IBS operation sampling
was retained, but this kernel did not attach a latency weight; no latency claim
is made from those samples.

The six-event branch/cache/TLB group in `counters-exact-3` scheduled at `0%`
and is rejected. It was split into the valid four-event groups above rather
than reported as multiplexed data.

Syscall code `267` (`SECP256K1_DOUBLE`) executed `600,576` times and implemented
doubling as a general scalar multiplication by two. Replacing it with direct
point addition (`P + P`) preserved exact outputs in focused equivalence tests
and all full-block proof runs. Five warm results were:

| Run | Preflight (s) | Shard-0 witness (s) | Combined (s) |
| ---: | ---: | ---: | ---: |
| 1 | 15.949864257 | 2.08 | 18.029864257 |
| 2 | 15.942025669 | 2.06 | 18.002025669 |
| 3 | 15.986692200 | 2.06 | 18.046692200 |
| 4 | 15.968778610 | 2.05 | 18.018778610 |
| 5 | 15.902847085 | 2.06 | 17.962847085 |
| Median | 15.949864257 | 2.06 | 18.018778610 |

The retained combined median is `8.902%` faster than accepted ABI-6 and
`25.908%` faster than control. Every run preserved the canonical hash,
`994,896,527` guest instructions, `3,979,586,112` cycles, tape usage/capacity,
zero overflow/callbacks, and verified shard 0. Raw results are under
`.codex-results/aot-cpu-20260728/`.

## Outstanding promotion check

The circuit-aware WITGEN=0 sanity for block `23587691` could not start. Its
cached input (`block_data/input/1/23587691.bin`, dated 2025-12-11) is
incompatible with the current bincode schema and fails decoding with
`unexpected string`. No RPC environment is configured to refresh the cache.
The stale cache was left untouched; older proof artifacts were not presented
as validation of this checkout. After refreshing that cached input, rerun
execute sanity with `CENO_MAX_CELL_PER_SHARD=2684354560`, require the calibrated
two-shard shape, then prove shard 0.

## 2026-07-31 frontend layout and hot/cold compaction

Two profile-guided frontend candidates were evaluated against a fresh detached
control built from Ceno `6ab15eb0`. The benchmark used block `25580200`, cached
`--chain-id 1` input, `jemalloc,gpu,aot`, `CUDA Backend Enabled`, CPU 0,
`CENO_GPU_WITGEN=0`, and a `268435456`-cell shard limit. Control and candidate
artifacts used isolated caches. The accepted production baseline remains ABI 6;
ABI 7 and ABI 8 below are experiments, not promotions.
The experimental ABI numbers are local cache generations and reuse numbers
from earlier, already-reverted experiments; they are not a global sequence.

### Whole-image weighted chaining (ABI 7): rejected

ABI 7 retained all 35,280 native blocks, persisted a dense execution/edge
profile and deterministic emission order, and converted the most common static
successor into layout fallthrough. It observed 22,910 blocks and raised weighted
adjacent-edge coverage to `65.49%`. The preflight artifact was `191,101,064`
bytes, only `0.056%` smaller than the `191,207,560`-byte control.

Five warm preflight runs were `15.059568392`, `15.666082467`, `16.219537943`,
`16.212812992`, and `16.318228826` seconds. Their median was
`16.212812992 s`, just `0.233%` below the matched control median of
`16.250643596 s`, far short of the required `5%`. The first run was an isolated
low outlier and was not used as evidence of a durable gain. Hash, instruction
and cycle totals, fallback categories, tape behavior, all 657 shard boundaries,
FullTracer behavior, witness records, and shard-0 proof verification passed.
The frequency/profile/cache infrastructure remains useful, but whole-image
reordering is not an accepted optimization.

### PC-ordered 99.5% hot section (ABI 8): rejected and reverted

ABI 8 selected 5,550 compilable blocks in deterministic count/PC order. They
covered `99.500%` of trained native guest instructions; entry was always
included. Only those blocks received full native bodies. Other PCs used shared
typed fallback stubs, while canonical PC order remained the cost-table index
order. The identical hot set and layout digest were reused by FullTracer.

The footprint mechanism worked:

| Metric | ABI 6 control | ABI 8 compact | Change |
| --- | ---: | ---: | ---: |
| Preflight artifact | 191,207,560 B | 64,861,848 B | -66.08% |
| `.text` | 179,959,034 B | 62,546,297 B | -65.24% |
| RX 4 KiB pages | 43,936 | 15,271 | -65.24% |
| Fallback | 2,036,036 (0.20%) | 6,999,492 (0.70%) | +0.499 point |

However, the five warm compact runs were `16.406653067`, `16.431885731`,
`16.435857913`, `16.412481100`, and `16.481780341` seconds. The median was
`16.431885731 s`, a `1.115%` regression from control and well above the
`15.438111416 s` promotion ceiling. Shard-0 witness medians stayed at
approximately `2.04 s`.

The candidate also failed exact planning semantics. One of 657 shard boundaries
moved from cycle `770,893,916` to `770,893,936`. A cold compiled block had been
replaced by per-step Rust recovery, changing the accepted block-atomic planning
decision even though the final hash, `994,896,527` guest instructions,
`3,979,586,112` cycles, tape usage `25,334,834 / 28,492,288`, and zero-overflow
state remained unchanged. FullTracer shard 0 added 10,348 Rust transitions.
Cold ECALL recovery also left a stale recovery reason for following dynamic
steps, producing bogus ECALL histogram keys; this is a diagnostic-category bug
in the shared-stub design.

### Matched phase-controlled profile

Fresh non-multiplexed counters enabled only by the `aot_execute` FIFO markers
show why the smaller image did not improve latency:

| AOT-only event | ABI 6 control | ABI 8 compact | Change |
| --- | ---: | ---: | ---: |
| Cycles | 71,669,404,678 | 72,015,887,986 | +0.48% |
| Instructions | 133,094,394,948 | 136,689,899,107 | +2.70% |
| Frontend-idle cycles | 23,157,546,212 | 22,197,566,151 | -4.15% |
| Branches | 17,853,602,231 | 18,524,188,783 | +3.76% |
| Branch misses | 854,584,620 | 836,738,903 | -2.09% |
| L1-I loads | 27,457,227,082 | 27,184,533,202 | -0.99% |
| L1-I misses | 369,327,258 | 308,579,377 | -16.45% |
| iTLB loads | 55,084,812 | 50,429,415 | -8.45% |
| iTLB misses | 24,401,010 | 19,704,179 | -19.25% |

Thus compaction genuinely reduced frontend pressure, but not by enough to pay
for 4,963,456 additional Rust fallback steps. Cycle sampling supports the same
conclusion. The compact candidate newly exposes
`ShardPlanBuilder::observe_modeled_step` (`0.69%`), `rv32im::step_fetched`
(`0.57%`), `SyscallEffects::finalize` (`0.52%`), and planner preview (`0.50%`)
above the 0.5% reporting threshold. Across the full reports, named recovery
symbols rise from `1.18%` to `2.84%`, while generated dispatch rises from
`1.30%` to `1.47%`. The large mapped-image reduction therefore overstates the
active working-set benefit: cold control pages were mostly not fetched, while
every new fallback performs real recovery and redispatch work.

The same samples keep the remaining generated-image work broad: named memory
regions account for `33.96%` of control cycles, block accounting for `15.41%`,
guest-body regions for `10.31%`, and dispatch for `1.30%`. These labels overlap
only at the generated-image total, not with one another. They justify the next
ablation sequence but do not yet identify a safe 5% implementation by
themselves.

### Stop decision and remaining optimization order

The ABI-8 layer was reverted, preserving the pre-existing ABI-7 profile
infrastructure in the worktree for inspection. All `ceno_emul` AOT tests then
passed (`58` passed, one ignored, plus two integration tests), and
`cargo check -p ceno_zkvm --features aot-x86_64` passed. No ABI-8 proof run was
started after the latency and boundary prerequisites failed.

Do not add hot chains on top of ABI 8, outline more bodies, or prototype huge
pages. Do not retry quadratic memory aggregation, address scratch caches, or
the earlier two-register block-local cache. The next bounded work is:

1. restore ABI 6 as the comparison baseline and retain dense profile counters
   only as non-production infrastructure;
2. measure semantic-floor, block-accounting, exact-memory, and full-planning
   modes with the same phase-controlled counter groups;
3. proceed only if an ablation identifies a component large enough to support a
   5% end-to-end win; prioritize reducing retired instructions per native hot
   block without adding Rust transitions;
4. require exact block-atomic shard recovery before any selective native-body
   removal is reconsidered.

Raw gate logs are under `.codex-results/aot-hot-layout-20260731/` and
`.codex-results/aot-hotcold-20260731/`. The matched follow-up counters and cycle
samples are under `.codex-results/aot-hotcold-20260731/profile/`.

## Artifacts

- Control logs: `.codex-results/aot-codesign-20260728/control/`.
- Preliminary tape-only logs: `.codex-results/aot-codesign-20260728/candidate/`.
- Accepted ABI-6 cold proof: `iterate3/train.log`.
- Accepted ABI-6 five-run gate: `iterate3/warm/run-1.log` through `run-5.log`.
- Profiling data: `iterate1/preflight-perf.data` and
  `iterate1/preflight-perf.log`.
- Rejected follow-on proof logs: `iterate4/train.log`, `iterate5/train.log`,
  and `iterate6/train.log`.
- Rejected ABI-7 layout gate: `.codex-results/aot-hot-layout-20260731/`.
- Rejected ABI-8 compaction gate and matched profile:
  `.codex-results/aot-hotcold-20260731/`.

## 2026-07-31 access/accounting attribution follow-up

The previously requested production, no-accounting, no-access, and semantic
floor modes were run with a corrected execute-only harness. The earlier
no-accounting attempt was discarded because its AOT key showed unbounded cells
and cycles; execute mode had not explicitly prepared AOT from its configured
`MultiProver`. The profiling-only binary fixes that mismatch and gives every
mode a distinct cache key. Tracked Ceno source was returned to the pre-existing
ABI-7 diff (`573` insertions, `38` deletions), and the benchmark GPU patch and
lockfile were restored.

Matched conditions were cached mainnet block `25580200`, `--chain-id 1`, CPU
0, `jemalloc,gpu,aot`, local Ceno and Ceno-GPU, `CUDA Backend Enabled`,
`CENO_GPU_WITGEN=0`, and a 268,435,456-cell limit. Five warm target-span times
were:

| Mode | Samples (seconds) | Median | Relative |
| --- | --- | ---: | ---: |
| control | 16.460038, 16.125476, 16.147906, 16.693458, 16.215945 | `16.215945` | control |
| no accounting | 13.785318, 13.763171, 13.876786, 13.642771, 13.867792 | `13.785318` | `-14.989%` |
| no access | 10.301287, 10.286441, 10.321740, 10.316279, 10.333707 | `10.316279` | `-36.382%` |
| semantic floor | 7.808289, 7.988797, 7.742888, 7.704857, 7.764178 | `7.764178` | `-52.120%` |

The access layer is the largest measured optimization pool: 5.900 seconds
with accounting retained, or 6.021 seconds above the semantic floor after
accounting is removed. Accounting contributes 2.431 seconds in isolation, or
2.552 seconds above the semantic floor after access is removed. These are trim
bounds, not additive forecasts.

Control and no-access both executed 994,896,527 AOT steps with 0.20% fallback.
The no-access run retained 3,979,586,112 guest cycles and capped-run shard
boundaries, but tape use collapsed from 25,334,834 to 581,366 events; it is
therefore intentionally witness-invalid. No-accounting changed shard shape to
394 and is also invalid. The semantic floor omits `PreflightTracer` step/cycle
maintenance, so its final tracer report contains only 2,036,036 fallback
steps; its 7.764-second median is solely an execution lower bound.

The corrected remote setting is `CENO_MAX_CELL_PER_SHARD=4500000000`.
ABI-7 AOT reported 35 shards, `994,896,527` instructions,
`3,979,586,112` cycles, `16,809,729 / 18,911,061` tape usage, zero overflow,
zero normal-path callbacks, and 0.20% fallback. Its cold target span was
`16.100995288 s`. Because AOT plans atomically at basic-block granularity,
matching interpreter boundary positions is explicitly not required. The proof
gate is internal consistency: preflight, cache reload, FullTracer replay,
witness generation, and proof verification must all consume AOT's same
35-boundary plan.

### Direction selected

Prioritize exact-access representation and tape emission, then sparse
cost-bucket updates. Start by separating static-register access from dynamic
memory access. The production candidate should batch block-static first/last
accesses, use shard epochs instead of repeated shard-start comparisons where
possible, and write exact tape events directly in order. At the remote setting
it must reproduce AOT's own 35-boundary plan across replay and proof paths
before latency is considered; interpreter boundaries may differ.

The semantic floor disproves an instrumentation-only route to the final goal:
`7.764 s` preflight plus approximately `2.04 s` for shard-0 replay/witness is
already about `9.8 s`. For a cached-block target below five seconds, the next
architectural prototype is an exact-input preflight cache containing the
validated shard plan and next-access tape. Its key must include the complete
input/hints digest, program digest, cost ABI, limits, and layout digest, and it
must fall back on every mismatch. Without that cache, a profile-selected
SSA/register-allocated native backend is required; frontend layout, outlining,
or huge pages cannot close the measured gap.

Raw logs are in `.codex-results/aot-ablation-20260731-v2/`; the diagnostic
binary SHA-256 is
`771ff25a3102c1366ab91a4b9a2f71d651c58b33799a6aeefdd03f6db6b818b2`.

## Native access checkpoint and sparse-accounting rejection (2026-07-31)

ABI-9 keeps the next-access cursor in `%rbx`, flushes/reloads it at every Rust
boundary, directly emits block-static events, batches release first-touch
updates, and removes `AOT_PREFLIGHT_HELPER_ACCESS`. On block `25580200` at
4.5B cells, five warm preflight samples were `16.392697`, `16.031515`,
`16.071714`, `16.136394`, and `16.064960` seconds (median `16.071714 s`),
`0.889%` below the ABI-7 control median. Exact tape usage (`16,809,729`),
instruction/cycle totals, 0.20% fallback, and all 35 boundaries matched. The
stage is retained as a native-only structural cleanup by explicit design
choice; logs are in `.codex-results/aot-native-tape-access-20260731/candidate/`.

The attempted per-chip bucket/threshold state was rejected: its median was
`17.343788 s` (`+6.96%` versus control) despite exact semantics. This shows
that adding hot threshold-state loads/branches is more expensive than the
existing branch-free bucket calculation for this workload. Raw rejected logs
are in `.codex-results/aot-native-tape-sparse-20260731/candidate/`.
