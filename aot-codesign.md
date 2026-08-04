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

## Block-level tape-capacity checkpoint (2026-07-31)

ABI-10 replaces per-event capacity loads/branches with one conservative native
block-entry guard. Warm samples were `14.279486`, `14.427257`, `15.202264`,
`15.797175`, and `15.454057` seconds (median `15.202264 s`), improving ABI-9
by `5.410%` and ABI-7 by `6.251%`. Tape, instruction/cycle totals, fallback,
and all 35 boundaries remain exact. The preflight artifact shrank to
`169,187,464` bytes. Shard-0 FullTracer positioning completed in `1.85 s` with
the same plan. Raw logs are in `.codex-results/aot-block-capacity-20260731/candidate/`.

The follow-up `prove-app --shard-id 0` run matched shard-0 replay again, then
was killed by SIGKILL at `118.59 s` while positioning/building shard 1
continuation state. No proof was serialized or verified, so this is an
explicitly incomplete proof gate rather than a correctness pass; see
`shard0-proof.log` and `shard0-proof.time` in that directory.

The result remains `1.958x` the matched semantic-floor median, so further work
must remove access/accounting instructions rather than only reorganize them.

## Same-bucket accounting branch rejection (2026-07-31)

A threshold-array-free ABI-11 experiment updated chip counts normally and
skipped trace/main/tower deltas when the padded bucket did not change. Its five
uncontended warm samples were `15.480995`, `15.391056`, `15.282137`,
`15.228894`, and `15.187410` seconds (median `15.282137 s`), a `0.525%`
regression from ABI-10. The expected hash, tape, instruction/cycle totals,
fallback, and all 35 boundaries remained exact, but the stage missed the 5%
gate and was reverted. Artifact size was `169,359,496` bytes. Raw logs are in
`.codex-results/aot-sparse-bucket-skip-20260731/candidate/`; overlapping
`warm2`/`warm3` samples are explicitly excluded.

## First-touch batching rejection (2026-07-31)

ABI-12 accumulated release-build first touches natively and published the
count only with cursor flushes. Warm samples were `15.560373`, `15.651944`,
`15.472060`, `15.547530`, and `15.540207` seconds (median `15.547530 s`), a
`2.271%` regression from ABI-10. Exact tape/first-touch totals, fallback, and
all 35 boundaries matched; artifact growth was `2.45%`. The stage was reverted
because a hot stack read-modify-write did not beat the original hot tracer
field update. Logs are in
`.codex-results/aot-first-touch-batch-20260731/candidate/`.

## Shard-local register mask (2026-08-03)

Matched ABI-10 trim profiling identified static-register access checks as the
larger exact-access component: the warm median fell from `14.372994 s` to
`11.890733 s` when those checks were diagnostically disabled (`-17.27%`),
compared with `13.870011 s` when dynamic-memory tracking was disabled
(`-3.50%`). These trims intentionally changed the tape and were used only to
rank implementation work; no diagnostic selector remains in production.

The ABI-11 design uses a 64-bit shard-local touched mask because the register
address space includes 32 architectural registers plus Ceno's internal x0
write sink. At native block entry, a single subset test skips all register
history checks if the block's compile-time first-register mask is already
touched. Otherwise the original address-ordered checks run unchanged and the
block mask is published only after completion. Reconstructing from
`latest_access`, resetting on shard changes, and synchronizing the mask with
the resident tape cursor make callback and fallback boundaries conservative.
Exact/fallback paths need not publish bits: a false negative performs extra
checks, whereas a false positive could lose an event and is forbidden.

The compact encoding's five cached warm samples were `13.875333`, `13.743991`,
`13.805855`, `14.056734`, and `13.820847` seconds (median `13.820847 s`,
`-3.84%` from matched ABI-10). Hash, instruction/cycle totals, exact
`16,809,729 / 18,911,061` tape usage, zero helpers/overflow, 0.20% fallback,
and all 35 boundaries matched. Forty-two AOT unit tests pass; the performance
probe remains intentionally ignored.

The generated preflight object is `178,374,792` bytes (`+5.43%`). Per the
2026-08-03 review, object size and cold setup are no longer rejection criteria;
warm end-to-end `create_proof` time is authoritative. This is therefore a
provisional code checkpoint until the proof workflow comparison completes.
Raw control/trim logs are in
`/home/wusm/data/codex-aot-access-profile-20260803/`, and candidate logs are in
`/home/wusm/data/codex-aot-register-mask-v3-20260803/`.

### Warm proof result

Ceno `983eda5787cc5ee0a3d056a37a2bb67c352d2da0` is retained. The cold remote
run successfully produced a root proof but paid `201.878500 s` for the new
ABI's one-time AOT artifact. On the repeated run, both preflight and FullTracer
artifacts hit cache and loaded in `166.151 us`. Warm app `create_proof` was
`174.800661 s` versus ABI-10's `176.375837 s` (`-0.89%`), and total GPU
`create_proof` was `191.620974 s` versus `193.357967 s` (`-0.90%`). Preflight
was `15.685272 s` versus `16.168197 s`; shard-0 proof was `4.65 s` versus
`4.69 s`. The 96,182-byte root proof was present.

Workflow: [30805152620](https://github.com/scroll-tech/ceno-reth-benchmark/actions/runs/30805152620).
Published result: [mainnet25580200-20260803-182238](https://github.com/scroll-tech/ceno-reth-benchmark/blob/gh-pages/benchmarks-dispatch/refs/heads/feat/opt_aot/mainnet25580200-20260803-182238_summary.md).
The remote ABI-10 and ABI-11 ready inputs differed slightly in instruction,
cycle, tape, and boundary counts, so the remote percentage is an operational
comparison; exact same-input tape equality is established by the local
CPU-pinned validation. Both remote candidate runs were mutually identical,
had 35 self-consistent boundaries, the expected block hash, zero
overflow/helpers, and 0.20% fallback.

## 2026-08-03 local AOT hardware profile and direction review

This checkpoint used only cached block `25580200` in preflight-only
`--mode execute`; no CI or proof was launched. The input was frozen at
21,842,724 bytes with SHA-256
`aa46af2e2365057d626de51cfd8c9f415c77ad30bedce1b87e42259a5a8799c3`, and
both variants used the same copied 3,392,164-byte guest ELF with SHA-256
`26f7581d5b37127e5014b4b5e22f97782c8fee0b14236d23ed3bb008c3497438`.
The expected block hash was
`34439c597563024690ce3c91a082c34507569c7e18cc4d1b3b68550b791a2773`.

The revisions were Ceno ABI-11
`983eda5787cc5ee0a3d056a37a2bb67c352d2da0`, XMM ABI-12
`4f5dfeddf67d42f7bce536f478d22ab336ded691`, its revert
`20ee551772798c4b231c1f7d7abaad0509fd1d39`, benchmark `63713a1e` (input
freeze `8be9c24f`), and CUDA HAL
`996ef2a1c1f5648d8ae42b085f630ec84a514d7b`. Reproducibility material is in
`/home/wusm/data/codex-aot-hw-20260803/`, with separate source worktrees,
targets, and caches for `abi11` and `xmm`. Each was path-patched to its Ceno
tree and `/home/wusm/rust/ceno-gpu/cuda_hal`, then built with:

```console
cargo build --release --features jemalloc,gpu,aot --bin ceno-reth-benchmark-bin
```

The AMD Ryzen 9 5900XT host ran Linux 6.8.0-78 and perf 6.8.12 with
`perf_event_paranoid=1`, `schedutil`, boost and SMT enabled, and
`nmi_watchdog=1`. Every run was pinned to CPU 0 with `taskset -c 0`; sibling
CPU 16 remained online. One cold artifact build was excluded, followed by one
accepted warm sample per variant:

```console
taskset -c 0 env RUST_LOG=info CENO_MAX_CELL_PER_SHARD=4500000000 \
  CENO_AOT_CACHE_DIR=/home/wusm/data/codex-aot-hw-20260803/cache/VARIANT \
  target/release/ceno-reth-benchmark-bin --mode execute \
  --block-number 25580200 --chain-id 1 \
  --input-path /home/wusm/rust/ceno-reth-benchmark/block_data/input/1/25580200.bin
```

Both variants reported `CUDA Backend Enabled`, `994,896,527` instructions,
`3,979,586,112` cycles, tape `16,809,729 / 18,911,061`, `2,036,036`
fallbacks (`0.20%`), zero overflow/helpers, and identical 35 boundaries.
ABI-11 took `14.812157178 s`; XMM took `14.815914339 s` (`+0.025365%`). XMM
grew the preflight artifact from 178,374,792 to 214,960,264 bytes (`+20.510%`)
and replay from 156,419,000 to 228,967,352 bytes (`+46.381%`).

### Phase-gated PMU evidence

`run_phase_perf.sh` combines `CENO_CPU_PROFILE_PHASES=1` with the perf-control
FIFO and `--delay=-1`, counting only `aot_execute`. The first run with the
profile-specific cache key was rejected as cold and kept at
`raw/{abi11,xmm}-core-rejected-cold-profile.*`. Accepted groups were run once,
were non-multiplexed, and reported `100.00%` scheduling. A group using the
perf metrics `l1_itlb_misses` and `l2_itlb_misses` was rejected because those
names were not schedulable PMU events.

| aot_execute counter | ABI-11 | XMM ABI-12 | XMM delta |
|---|---:|---:|---:|
| cycles | 62,919,143,465 | 62,486,608,242 | -0.687% |
| instructions | 129,197,360,416 | 128,270,227,090 | -0.718% |
| IPC | 2.0534 | 2.0528 | -0.029% |
| branches | 16,689,138,303 | 16,697,204,624 | +0.048% |
| branch misses | 676,748,456 | 670,689,489 | -0.895% |
| branch-miss rate | 4.055% | 4.017% | -0.038 pp |
| frontend-stalled cycles | 16,307,432,064 | 16,401,057,582 | +0.574% |
| frontend stalls / cycles | 25.918% | 26.247% | +0.329 pp |
| L1-I load misses | 275,229,840 | 413,495,064 | +50.236% |
| iTLB load misses | 17,899,541 | 26,167,204 | +46.189% |
| cache misses | 2,363,894,042 | 2,958,150,273 | +25.139% |
| dTLB loads | 40,675,145 | 37,338,887 | -8.202% |
| dTLB load misses | 1,435,503 | 1,267,585 | -11.698% |
| dTLB miss rate | 3.529% | 3.395% | -0.134 pp |
| AMD `ic_stall_any` | 24,756,233,915 | 25,809,024,846 | +4.253% |
| AMD `ic_stall_any` / cycles | 39.346% | 41.303% | +1.957 pp |
| AMD `ic_stall_dq_empty` | 165,045,990 | 116,667,131 | -29.312% |
| AMD cacheable I-cache reads | 6,302,454,698 | 6,862,538,850 | +8.887% |
| AMD load dispatches | 38,210,043,563 | 34,826,387,595 | -8.855% |

The generic dTLB events duplicated their AMD alias values. Because each group
is a separate run, absolute counts across rows are not simultaneous. The
direction is nevertheless clear: XMM removed only about 0.7% of host cycles
and instructions, while L1-I misses rose 50.24%, iTLB misses 46.19%, total
cache misses 25.14%, and AMD instruction-cache stalls 4.25%.

Phase-gated 499 Hz `cycles:u` sampling produced about 7,000 samples per
variant with zero loss. Raw data and reports are
`raw/{abi11,xmm}-cycles.{data,report.txt}`, and annotations are
`raw/{abi11,xmm}-hot-block.annotate.txt`. The generated image accounts for
78.18% / 78.28% of samples, the host binary 18.29% / 18.52%, libc 2.41% /
2.19%, and unknown DSOs 1.12% / 1.01%. Direct fallback/callback bodies are
0.28% / 0.27%, direct synchronization/accounting callback helpers at most
0.01% / 0.08%, and the generated dispatcher 1.45% / 1.36%. These are direct,
not inclusive, shares because call graphs were disabled. The hottest named
host function was `rustsecp256k1_v0_10_0_modinv64` (8.51% / 8.32%); the
hottest generated block was `ceno_aot_bb_080d4a74_memory_080d4a84` (2.06% /
2.03%). Although XMM annotation contains `pinsrd`, no register-value movement
or synchronization site is near 5% of the full profile.

### Why this is not OpenVM, and the ranked follow-up

OpenVM `494feec4aacaa83fcce7925d3727741b7a055875` uses all 16 XMM registers,
packs two RV32 registers per XMM with `movq` / `pinsrq`, maps x10-x15 to
`r10d`, `r11d`, `r9d`, `r8d`, `ebp`, and `r13d`, synchronizes at entry/exit,
and emits instruction-specific assembly. Rejected Ceno ABI-12 instead used
XMM4-XMM11, four 32-bit lanes per register, no hot GPR overrides, general
`pextrd` / `pinsrd`, much larger generated objects, and synchronization while
preserving tape, first-touch, block-accounting, and shard semantics around
roughly 2.04 million fallback/callback transitions. The designs are not
equivalent.

The previously reported `-17.27%` trim removed static-register
history/latest-access checks, not register-array loads and stores. It changed
the tape by only 669 events, so it is evidence for access tracking rather than
residency. Directions rank as follows:

1. static-register history/latest-access checks (`-17.27%` non-faithful trim),
   conditional on a faithful event-order/latest-use mechanism;
2. shard/accounting updates (`-14.989%` non-faithful capped trim), noting that
   faithful sparse designs regressed by `+0.525%` to `+6.96%`;
3. register tape-event emission (`0.889%` direct-emission gain; first-touch
   batching `+2.271%` regression; only 669 static events removed);
4. incomplete touched-mask paths, as the next bounded diagnostic, while the
   existing faithful touched mask reaches only `3.84%`.

No production change is retained. A new residency design is barred unless
hardware attribution finds at least 5% of preflight cycles in register-value
movement and identifies a viable synchronization/layout recovery. Any other
candidate must improve the single warm local preflight by at least 5%, have
corroborating cycle/instruction/cache deltas, and preserve every exact
semantic invariant before a later warm proof checkpoint.

## 2026-08-03 accounting and exact-access follow-up

Fresh label-only ABI-11 profiling at Ceno `983eda57` separated the previously
combined accounting/register-entry symbol. A phase-gated 499 Hz `cycles:u`
sample with 7K samples and zero loss attributed `15.91%` exclusively to shard
accounting, `0.93%` to register latest-access commits, `0.13%` to plan commits,
`0.08%` to block-entry register checks, and less than `0.01%` to block-entry
tape appends. Guards were `3.62%`, guest bodies `14.24%`, and memory bodies
`36.35%`. The earlier trim ranking remains useful: static-register tracking
was the largest removable access component (`-17.27%`), followed by the
non-faithful accounting ceiling (`-14.989%`); direct tape emission had only a
`0.889%` gain.

An untimed generated-code diagnostic counted `442,923,441` chip-contribution
updates. Only `22,173` crossed a padded cost bucket, so `442,901,268`
(`99.994994%`) stayed in the same bucket. This cleared the frequency gate, but
the faithful ABI-12 fast path did not clear the performance gate: it always
updated counts and skipped trace/main/tower work on equal buckets, yet its
single warm time was `14.594251271 s` versus `14.812157178 s` for ABI-11, only
`1.471%` lower. It was reverted and its ABI-12 artifact was rejected.

The retained candidate instead extends block-atomic static-register tracking
to adaptive exact-access blocks. Dynamic-memory accesses remain exact per
step; static-register first accesses move to block entry and latest-access
commits move to block exit. The AOT ABI is bumped from 11 to 13; ABI 12 was
already used by the rejected XMM experiment. The single warm cache-hit
preflight was `12.407317827 s`, a `16.236%` reduction from the
ABI-11 control, and the excluded cold candidate run was `12.218908783 s` after
artifact generation.

Every cold/warm run preserved the block hash, `994,896,527` guest instructions,
`3,979,586,112` guest cycles, tape `16,809,729 / 18,911,061`, zero
overflow/helpers, the exact `2,036,036` fallback count and reason histogram,
and all 35 shard boundaries. All 60 `ceno_emul` unit tests passed (one perf
probe ignored), both integration tests passed, and `cargo check -p ceno_zkvm
--features aot-x86_64` passed.

All four-event counter groups scheduled at `100.00%`:

The counter groups were collected before the collision-safe ABI-only bump
from 12 to 13; the emitted candidate instructions are identical.

| `aot_execute` counter | ABI-11 | block-atomic exact access | Change |
|---|---:|---:|---:|
| cycles | 62,919,143,465 | 52,485,346,722 | -16.583% |
| instructions | 129,197,360,416 | 122,523,636,480 | -5.166% |
| IPC | 2.0534 | 2.3344 | +13.69% |
| branches | 16,689,138,303 | 15,112,248,663 | -9.449% |
| branch misses | 676,748,456 | 407,451,297 | -39.793% |
| frontend-stalled cycles | 16,307,432,064 | 12,224,761,483 | -25.036% |
| L1-I load misses | 275,229,840 | 291,331,311 | +5.850% |
| iTLB load misses | 17,899,541 | 17,158,303 | -4.141% |
| cache misses | 2,363,894,042 | 1,939,975,220 | -17.933% |

Status: **retained and committed locally as `2cae93f6`**. No CI or proof was
run, as required by the local 5% gate. Diagnostic counters and the rejected
same-bucket fast path are absent from the retained source. Commands, logs,
counter CSVs, symbol report, isolated sources/targets/caches, and the result
manifest are under
`/home/wusm/data/codex-aot-accounting-20260803/`.

### Access-history coarsening principle

This optimization succeeds because static-register history has block-boundary
semantics even though the old implementation maintained it at instruction
granularity. In the old exact path, every register-bearing instruction loaded
the history context and, for each static operand, loaded the previous cycle,
stored the new cycle, tested first touch, compared with the shard start, and
branched around event emission. Almost every execution produced no tape event;
the no-static-register trim changed the tape by only 669 entries. Its
`-17.27%` result measured repeated negative decisions and intermediate state
publication, not tape-copy bandwidth or guest-register movement.

For a block admitted atomically by the shard planner, only these register
observations escape the block:

- the first access to a register, which determines a first-touch or incoming
  cross-shard event;
- the last access to a register, which is the latest cycle observed by the
  next block.

The compiler already knows both subcycles. Emitting the first-access checks
once at block entry and the last-cycle stores once at block exit is therefore
a lossless reduction. No shard boundary can occur between them because a
failed budget check splits before the block. Static register addresses cannot
alias dynamic memory, whose accesses deliberately remain exact per step.
Fallback and exceptional paths retain their original exact handling, and a
speculatively rejected block is reset before execution.

The PMU signature matches this model: retired instructions `-5.166%`, branches
`-9.449%`, branch misses `-39.793%`, frontend stalls `-25.036%`, cache misses
`-17.933%`, cycles `-16.583%`, and IPC `+13.69%`. The large cycle benefit is
thus caused by removing poorly predicted, dependency-chained metadata work
from nearly one billion steps. It is not caused by fewer tape events and it is
not register-value residency. The new block-entry/latest-commit symbols occupy
only `0.08%`/`0.93%` because they measure the cheap replacement; the removed
exact operations were inlined into guest and memory instruction bodies.

There is a deliberate frontend tradeoff. The ABI-13 preflight object is
`198,666,816` bytes versus `178,374,792` for ABI-11 (`+11.376%`), and L1-I
misses rise `5.850%`. Yet the dynamic instruction/branch/cache reduction wins
decisively. This is a useful codesign rule: generated-code size is secondary
when static specialization removes much more frequently executed history
machinery, but it must remain a measured constraint.

Ceno commit `2cae93f6` records the retained implementation. A fresh local
current-branch comparison independently measured `14.769762030 s` ABI-11
versus `12.476584248 s` ABI-13 (`-15.526%`) with identical canonical hash,
994,896,527 guest instructions, 3,979,586,112 cycles, tape/fallback totals,
and 35 boundaries. The post-cleanup focused test result is 60 unit plus 2
integration tests passed. Artifacts are in
`/home/wusm/data/codex-aot-local-20260804/`.

General rule: when execution is already atomic at a larger semantic boundary,
replace repeated exact bookkeeping for statically known state with compiler
summaries of the first and last externally visible effects. Keep dynamic or
aliasable effects exact, preserve rollback at the boundary, and validate both
event totals and boundary state—not only final guest output.
