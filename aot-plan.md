# Ceno AOT Preflight Optimization Plan

## Objective

Reduce block `25580200` warm AOT preflight from approximately 20.3 seconds to
5.1 seconds without relying on guest-instruction reduction. This corresponds to
improving throughput from approximately 49 MHz to at least 195 MHz.

The one-second stretch target is a separate architectural objective:

| Target | Required speedup | Required throughput |
| --- | ---: | ---: |
| Current | 1x | 49 MHz |
| 5.1 seconds | 4x | 195 MHz |
| 1 second | 20.3x | 995 MHz |

At roughly four host CPU cycles per guest instruction on a 4 GHz CPU, a
one-second exact planning pass is not a realistic extension of the current
design. It requires eliminating the separate full-program planning pass.

## Baseline

- Block: `25580200`
- Block hash: `0x34439c597563024690ce3c91a082c34507569c7e18cc4d1b3b68550b791a2773`
- Guest instructions: `994,896,527`
- Warm AOT preflight: `20.29-20.76 s`
- Throughput: approximately `48.5 MHz`
- Fallback steps: `13,567,020` (`1.36%`)
  - memory guard: `12,063,285`
  - dynamic PC: `18,380`
- Ecall count: approximately `1.49M`
- Compiled program: `35,280` blocks and `318,273` reachable instructions
- Observed generated artifacts: approximately `30-114 MB`

Fallback coverage is already high. Eliminating the remaining fallback alone
cannot provide a fourfold improvement. The primary targets are generated-code
quality, CPU frontend pressure, and per-instruction planning/accounting work.

## Current execution order

The implementation already has block cost planning and memory fast paths, so
production candidates are evaluated in this order:

1. true block-atomic register-only bookkeeping;
2. profile-guided hot/cold AOT layout;
3. block-local guest register caching;
4. memory-block aggregation and specialization;
5. removal of the standalone planning pass.

The accepted production path now uses a same-block-trained artifact for block
`25580200`. Keep an independent candidate when its five-run combined median
improves by at least 1%, or when it materially reduces host instructions or
memory without regressing combined latency by more than 1%. The 15% threshold
applies to the final improvement over control, not to every incremental step.
Checkpoint accepted improvements independently, return failed experiments only
to the latest checkpoint, and retain their profile results.

## Candidate reports

### 2026-07-27: true block-atomic register-only bookkeeping — rejected

The candidate computed first and last static register-access cycles, retained
first-touch side effects at block entry, and deferred final latest-access,
cycle, pending-step, planner-step, executed-step, PC, and tracer updates to
block exit. It also changed the AOT cache ABI from 3 to 4 while under test.

- Ceno control revision: `a6b43e31` (`feat(aot): cache full-coverage preflight artifacts`).
- Benchmark revision: `3184f8a3981b875a77f0019e4296eefb56e632c9`.
- Host: AMD Ryzen 9 5900XT; runs pinned to logical CPU 0.
- Training block: `25607900`; test block: `25580200`; cached local inputs.
- Control artifact: ABI 3, 114,198,496 bytes, SHA-256
  `7a7541271c93796c0966c46e022ee70229b2ad854391622f0d136ba14381e56d`.
- Candidate artifact: ABI 4, 111,302,624 bytes, SHA-256
  `da4118a6e781e4d0cf63544bb51b864824f6aab9bde30751a572cc0d13a08353`.
- Control warm times: `28.064049085`, `27.729123203`, `27.635614255`,
  `27.618055506`, `27.771258096` seconds; median `27.729123203 s`
  (`35.879120 MHz`).
- Candidate warm times: `27.810082494`, `27.760569455`, `27.797610789`,
  `27.678472195`, `27.838851037` seconds; median `27.797610789 s`
  (`35.790721 MHz`).
- Incremental result: `-0.068487586 s` gain, or `-0.246379%`; artifact size
  fell `2.535823%`.
- Correctness observed in every warm run: block hash
  `0x34439c597563024690ce3c91a082c34507569c7e18cc4d1b3b68550b791a2773`,
  `994,896,527` instructions, `3,979,586,112` internal cycles, identical
  fallback totals, and identical 140-boundary shard plan. The boundary-line
  digest for both variants was
  `eadc0f23ae2b18f54b538e9b6fe455cce60534a04c18aeacd5ed6ad7ff41bbbf`.
- The requested cross-block training artifact was substantially colder than
  the historical baseline: fallback was `197,103,219` steps (`19.81%`), not
  `1.36%`. This explains the higher absolute control time but does not affect
  the same-artifact incremental comparison.
- Focused validation before measurement: all 39 AOT tests passed, including
  arithmetic, branches, `JAL`, division edge cases, partial `max_steps`,
  repeated first/last register accesses, memory fallback, and shard splits.
- ShardRAM replay was not run after the performance gate failed; no candidate
  code is being retained.

Status: **rejected and reverted**. The candidate missed the 15% gate and was a
small regression. Ceno source and the AOT ABI are restored to the control
revision. The next production candidate is profile-guided hot/cold AOT layout;
its first requirement is to resolve the unexpectedly cold cross-block profile
without allowing fallback above 3%.

Raw logs and binaries are under
`/tmp/ceno-aot-stage1-baseline.We63zN/` for this measurement session.

### 2026-07-27: profile-guided hot-block layout — rejected

This Stage 2 slice recorded exact block-entry frequencies during coverage
training, persisted them in versioned cache metadata, and emitted native blocks
in descending trained frequency while retaining the address-sorted dispatch
tree. Fallback and error paths were already outlined after native blocks. The
candidate changed the cache ABI from 3 to 4 and cache metadata from v1 to v2
while under test.

- Ceno control revision: `a6b43e31a469e0ce47796bda64ab239bb0e670cb`.
- Benchmark revision: `3184f8a3981b875a77f0019e4296eefb56e632c9`.
- Candidate binary SHA-256:
  `7aa6a01659a6eb6571fdab406c4369638672726988ed8a2786208c5f853ef0df`.
- Host: AMD Ryzen 9 5900XT; runs pinned to logical CPU 0.
- Training block: `25607900`; test block: `25580200`; cached local inputs.
- Control artifact: ABI 3, 114,198,496 bytes, SHA-256
  `7a7541271c93796c0966c46e022ee70229b2ad854391622f0d136ba14381e56d`.
- Candidate artifact: ABI 4, 114,214,880 bytes, SHA-256
  `44eb8e56b6ebba7eefde3bbce253e71b1d12a4e90f0b606b6777160c50956b5b`.
- Control warm times: `28.064049085`, `27.729123203`, `27.635614255`,
  `27.618055506`, `27.771258096` seconds; median `27.729123203 s`
  (`35.879120 MHz`).
- Candidate warm times: `27.549595195`, `27.759732997`, `27.482966268`,
  `27.579602833`, `27.654182155` seconds; median `27.579602833 s`
  (`36.073635 MHz`).
- Incremental result: `0.149520370 s` lower median, a `0.542141%`
  throughput speedup (`0.539218%` elapsed-time reduction). Artifact size grew
  by 16,384 bytes (`0.014347%`) instead of shrinking.
- Correctness observed in every warm run: block hash
  `0x34439c597563024690ce3c91a082c34507569c7e18cc4d1b3b68550b791a2773`,
  `994,896,527` instructions, `3,979,586,112` internal cycles, identical
  fallback totals, and identical 140-boundary shard plan. The boundary-line
  digest for all control and candidate runs was
  `eadc0f23ae2b18f54b538e9b6fe455cce60534a04c18aeacd5ed6ad7ff41bbbf`.
- Fallback remained `197,103,219` steps (`19.81%`): `183,807,919` dynamic-PC
  and `11,999,296` memory-guard steps. This misses the Stage 2 requirement of
  less than 3% fallback and leaves physical block reordering unable to affect
  nearly one fifth of execution.
- Focused validation before measurement: all 39 runnable AOT tests passed
  (one ignored), including new frequency counting, cache-metadata round-trip,
  and hot-first emission tests.
- ShardRAM replay was not run after the performance gate failed; no candidate
  code or ABI change is being retained.

Status: **rejected and reverted**. The layout-only candidate missed the 15%
gate by a wide margin and did not reduce artifact size. Ceno source, cache
metadata, and the AOT ABI are restored exactly to the control revision. Cold
stubs and transition fallthrough were not added after this independently
measured prerequisite failed.

Raw logs, binary, artifact, and metadata are under
`/tmp/ceno-aot-stage2-hot-layout/` for this measurement session.

### 2026-07-27: two-register block-local guest cache — rejected

This Stage 3 candidate ranked guest-register accesses within each register-only
native block and cached the two most-used registers in callee-saved x86
registers. Cached writes were flushed once before successor dispatch. The
existing whole-block `max_steps` guard kept partial blocks on fallback, and
memory blocks, dynamic jumps, ecalls, traps, and unsupported instructions were
unchanged. The cache ABI changed from 3 to 4 while under test.

- Ceno control revision: `a6b43e31a469e0ce47796bda64ab239bb0e670cb`.
- Benchmark revision: `3184f8a3981b875a77f0019e4296eefb56e632c9`.
- Candidate binary SHA-256:
  `ac98c26584bc097b7dd99cddb9cad6b3a5ff68c84c9a7d87b84668a84e9d0960`.
- Host: AMD Ryzen 9 5900XT; runs pinned to logical CPU 0.
- Training block: `25607900`; test block: `25580200`; cached local inputs.
- Control artifact: ABI 3, 114,198,496 bytes, SHA-256
  `7a7541271c93796c0966c46e022ee70229b2ad854391622f0d136ba14381e56d`.
- Candidate artifact: ABI 4, 113,469,408 bytes, SHA-256
  `c9dc219f1c5d7286cb63891dd731a5600e7380b65b070ef75fe310cdfe96619b`.
- Control warm times: `28.064049085`, `27.729123203`, `27.635614255`,
  `27.618055506`, `27.771258096` seconds; median `27.729123203 s`
  (`35.879120 MHz`).
- Candidate warm times: `27.320938404`, `27.395941663`, `27.300353141`,
  `27.486824501`, `27.515209835` seconds; median `27.395941663 s`
  (`36.315471 MHz`).
- Incremental result: `0.333181540 s` lower median, a `1.216171%`
  throughput speedup (`1.201558%` elapsed-time reduction). Artifact size fell
  by 729,088 bytes (`0.638439%`).
- Correctness observed in every warm run: block hash
  `0x34439c597563024690ce3c91a082c34507569c7e18cc4d1b3b68550b791a2773`,
  `994,896,527` instructions, `3,979,586,112` internal cycles, identical
  fallback totals, and identical 140-boundary shard plan. The boundary-line
  digest for all control and candidate runs was
  `eadc0f23ae2b18f54b538e9b6fe455cce60534a04c18aeacd5ed6ad7ff41bbbf`.
- Fallback remained `197,103,219` steps (`19.81%`): `183,807,919` dynamic-PC
  and `11,999,296` memory-guard steps.
- Focused validation before measurement: 37 AOT tests passed (one ignored),
  including cache ranking/write tracking and differential register state,
  branches, `JAL`, division edges, `max_steps`, memory fallback, and shard
  splits.
- ShardRAM replay was not run after the performance gate failed; no candidate
  code or ABI change is being retained.

Status: **rejected and reverted**. The candidate was correct and modestly
smaller, but its `1.216171%` speedup missed the 15% gate. Ceno source and the
AOT ABI are restored exactly to the control revision. The result indicates
that caching only two guest registers in register-only blocks cannot offset
the remaining per-instruction accounting and the cold-profile fallback share.

Raw logs, binary, artifact, and metadata are under
`/tmp/ceno-aot-stage3-register-cache/` for this measurement session.

### 2026-07-27: exact interpreter-aligned shard boundaries — rejected

Three increasingly conservative variants tested exact per-step recovery when
a block-level cost candidate crossed a shard limit. The first rolled back the
speculative block cost and interpreted the boundary-block suffix. It retained
the control's performance (five-run median `27.463542034 s`) but still differed
from the interpreter at 72 of 141 boundary entries, with maximum displacement
`559,904` cycles. Recomputing the aggregate cost did not change the result.

Profiling and a focused regression then identified mixed accounting in dynamic
memory blocks: the block descriptor precharged the complete opcode mix while a
runtime memory guard could execute an instruction through the exact planner.
Suppressing that second charge made the first shard exact but left 73 boundary
differences, with maximum displacement `352,168` cycles, showing that exact
intermediate planning cannot be reconstructed from only the block's final
aggregate cost.

The final diagnostic disabled adaptive block costing only for memory blocks
whose address base is written inside the block. Static-address memory blocks
kept their block-entry guarded native path. This produced the exact interpreter
boundary digest
`f2d22fe9f1b936f7ac4c6ca1ce9bbf3e0ac1e1abee3ce2c40658dc7ce7746a40`
in every run, with the expected block hash, `994,896,527` instructions, and
`3,979,586,112` cycles. However, it moved `327,757,412` instructions to
exceptional interpreter recovery: total fallback rose from `19.81%` to
`52.28%`.

- Control warm times: `28.064049085`, `27.729123203`, `27.635614255`,
  `27.618055506`, `27.771258096` seconds; median `27.729123203 s`.
- Exact diagnostic warm times: `40.772083394`, `40.747436623`,
  `40.744332206`, `40.422024729`, `41.019097444` seconds; median
  `40.747436623 s`.
- Result: `46.948161%` elapsed-time regression and `31.948791%` lower
  throughput.
- Artifact: ABI 4, `61,786,080` bytes, SHA-256
  `4b5b102819f3363982479fe7da153e6fc87e3d1a01d95aa7d71880bf08f6a354`.
- Focused tests covered cell- and cycle-limit cuts inside blocks, exact
  next-access maps and predicted costs, and dynamic-memory fallback planning.

Status: **rejected and reverted**. No code or ABI change is retained and no
commit was created. The key follow-up is a native exact planner for dynamic
memory blocks: it must evaluate intermediate instruction costs and split inside
the block without moving hundreds of millions of guest semantics back to Rust.

Raw artifacts and logs are under
`/tmp/ceno-aot-exact-boundary-no-adaptive-memory/`. Earlier diagnostic variants
are under `/tmp/ceno-aot-exact-boundary/`,
`/tmp/ceno-aot-exact-boundary-recompute/`, and
`/tmp/ceno-aot-exact-boundary-planned-guard/`.

#### Follow-up: reachable memory-instruction block splitting — rejected

To retain native memory semantics without mixed aggregate/exact planner state,
the follow-up subdivided each already-reachable AOT block at every load/store.
An initial implementation incorrectly promoted every memory PC in the program
to a graph root; it generated 3.13 GB of assembly and was stopped after 4.5
minutes. The corrected implementation split only the normal coverage-reachable
blocks.

The corrected artifact matched the interpreter boundary digest exactly and
reduced fallback to `190,077,715` steps (`19.11%`), including `4,424,870`
memory-guard steps. However, block count grew from `20,422` to `97,229`,
artifact size grew to `172,248,280` bytes, and compile/load took `91.51 s`.
The added dispatch and block-entry planner work outweighed the 7.03M-step
fallback reduction.

- Warm times: `33.993101320`, `34.137434368`, `34.447371403`,
  `34.520391522`, `34.285157983` seconds; median `34.285157983 s`.
- Versus the `27.729123203 s` control median: `23.643138%` elapsed-time
  regression and `19.122078%` lower throughput.
- Artifact SHA-256:
  `1ba981e6c3c26a60da8ac33fc752532ba1af4166b11458e20a90bcf99a72e86f`.
- All 36 active AOT tests passed; one performance probe remained ignored.

Status: **rejected and reverted**. No commit was created. Future exact planning
must not turn each memory instruction into a dispatch boundary. A feasible
design needs an in-block cost checkpoint or a hoisted region guard while
keeping the original coarse block layout.

Raw logs and artifacts are under
`/tmp/ceno-aot-exact-boundary-reachable-memory-split/`; the stopped root-expansion
diagnostic is under `/tmp/ceno-aot-exact-boundary-memory-split/`.

#### Follow-up: measured selective overflow-block replay — rejected

The next experiment replaced the unproven assumption that boundary blocks are
rare with an overflow-branch counter. On the cross-trained control, target
block `25580200` entered the aggregate-overflow branch 67 times, covering only
6,469 guest instructions (`0.000650%` of 994,896,527). This establishes that
the expensive exact path can be a small runtime-selected subset; it is not a
claim about a static class of blocks.

Three increasingly narrow implementations were then tested:

1. Roll back an overflowing aggregate and interpret that block exactly, while
   treating every fallback PC below the aggregate block end as already planned.
   This rule was too broad and produced only 138 shards.
2. Restrict the credit to a counted lexical suffix after a memory guard. This
   restored 140 shards, but still moved boundaries because the generated memory
   slow path returns to native execution rather than interpreting that suffix.
3. Credit only the instruction whose actual fallback reason is
   `MEMORY_GUARD`, while replaying aggregate-overflow blocks exactly. This
   matched all 140 interpreter shard boundaries in every run. The textual
   boundary-list SHA-256 was
   `ebc512873c15565c9f5e630d4a89207d343414b68280e9549c8fda3e1de667ee`
   for both interpreter and candidate.

The final variant also preserved block hash
`34439c597563024690ce3c91a082c34507569c7e18cc4d1b3b68550b791a2773`,
`994,896,527` instructions, and `3,979,586,112` cycles. All 36 active AOT
tests passed (one performance probe remained ignored), including differential
cell-limit, cycle-limit, exact overflow-block, memory-recovery cost, and
next-access tests.

- Control median: `27.729123203 s`.
- Candidate warm times: `27.772084025`, `27.471505531`, `28.015400247`,
  `27.731269625`, `27.892802222` seconds.
- Candidate median: `27.772084025 s`.
- Result: `0.042960822 s` slower, a `0.154930%` elapsed-time regression.
- Candidate artifact: ABI 4, `114,874,336` bytes, SHA-256
  `2eb2e8b776b89e4c2174bce441b50cbe703c40d9538585e4e9dbb62ab91e7b2e`.

Status: **rejected and reverted**. The candidate proves selective exact replay
is feasible for correctness, but it cannot satisfy the roadmap's independent
15% performance gate. No commit was created. Raw profiling, build, artifact,
and run logs are under `/tmp/ceno-aot-boundary-subset-profile/` and
`/tmp/ceno-aot-selective-boundary-candidate/`.

During this experiment the 3.13 GB temporary assembly from the previously
stopped all-memory-root diagnostic was removed to free build space. Its logs
and report remain, but that assembly file can only be recovered by rerunning
the discarded diagnostic.

## 2026-07-27 profile-guided reprioritization

This profile supersedes the previous next-stage order. Three matched CPU-time
sampling profiles were collected with `gprofng`, pinned to logical CPU 0, using
cached block `25580200`. Hardware counters were unavailable because the host
sets `perf_event_paranoid=4`, so no branch, cache, or IPC claims are made.
Inclusive rows overlap and must not be summed.

| Configuration | Preflight | Fallback | Native AOT exclusive | `aot_exec_one` inclusive | Planner inclusive | Ecalls inclusive |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| ABI-3 cross-trained control | 27.920 s | 197.10M (19.81%) | 11.618 s | 13.650 s | 4.803 s | 4.123 s |
| ABI-4 two-register candidate | 27.378 s | 197.10M (19.81%) | 11.408 s | 13.249 s | 4.183 s | 4.113 s |
| ABI-3 target-trained diagnostic | 20.806 s | 13.57M (1.36%) | 14.440 s | 4.973 s | 0.440 s | 4.043 s |

Observed findings:

- The two-register cache changed only the native portion: native exclusive CPU
  fell by about 0.21 seconds in the sample, while fallback and ecall work were
  unchanged. This explains the measured 1.216% five-run gain.
- In the cross-trained control, Rust fallback accounts for 51.4% of AOT
  preflight inclusively. Its 183,807,919 dynamic-PC misses also force exact
  `observe_modeled_step` and `preview_modeled_chips` work.
- Training diagnostically on the target reduced fallback by 93.12% and reduced
  sampled preflight elapsed time by 25.48% (34.19% higher throughput). This is
  a controlled upper bound, not an acceptable production artifact under the
  cross-block training contract.
- Naive target-specific expansion grew the artifact from 114,198,496 to
  197,809,984 bytes (73.22%) and compilation took 108.67 seconds. Coverage must
  therefore be fixed without simply compiling a large target-specific clone.
- Ecalls remain about four seconds. Code 267 is `SECP256K1_DOUBLE` and occurs
  600,576 times; its `secp256k1_double` path alone was 2.822 seconds inclusive
  in the control profile. Code 65802 (`SECP256K1_ADD`) occurs 299,171 times.
- `FxHashMap` insertion is 1.731 seconds exclusive in the target-trained
  profile. `record_native_access_side_effects` is 1.201 seconds inclusive and
  inserts cross-shard next-access events keyed by prior cycle, including
  rehash/reserve work.

Correctness audit:

- The interpreter took 57.2 seconds and is the authoritative shard-planning
  oracle for this audit.
- Both AOT variants preserved the block hash, 994,896,527 instructions, and
  3,979,586,112 cycles, but neither preserved the interpreter shard plan.
- The cross-trained AOT differed at 87 of 141 boundary entries, with a maximum
  absolute displacement of 559,904 cycles. The target-trained AOT differed at
  89 entries, with a maximum displacement of 562,064 cycles.
- The cross-trained fallback histogram also omitted five ecall codes that the
  interpreter/target-covered path observed. `fallback_recovery_reason` retains
  the initiating fallback category until the next native leader, so recovered
  ecalls are classified under an earlier memory/dynamic reason.

Status: the current AOT baseline is fast but does **not** satisfy the roadmap's
exact-shard-boundary correctness gate. No further production optimization
should be accepted until this is repaired.

### Revised feasible order

1. **Repair exact boundary planning.** Add the interpreter boundary digest,
   next-access map, and ShardRAM replay as mandatory oracles. When a block-level
   candidate would cross a shard limit, execute that boundary block through an
   exact per-step native path so the split can occur inside the block; retain
   block aggregation everywhere else. Separately classify fallback statistics
   from the actual instruction rather than a stale recovery reason.
2. **Eliminate cross-profile dynamic-PC misses.** First add static roots for
   link-register return sites and ELF function entries, then measure the
   remaining miss PCs. If needed, persist a union of indirect targets across
   training inputs. Require less than 3% fallback without the 198 MB
   target-specific expansion. The measured coverage ceiling is large enough to
   pass the 15% gate.
3. **Replace hot next-access hashing.** Append compact
   `(previous_cycle, address, current_cycle)` events during execution and
   sort/group once at finalization, or pre-size/batch the existing structure.
   Validate identical next-access and ShardRAM output. Profile-guided trims
   should select the representation before production implementation.
4. **Reduce secp syscall transitions.** The useful target is the repeated
   affine parse/double-or-add/normalize/serialize sequence, especially field
   inversion, not the ecall histogram itself. Evaluate a constrained batched or
   scalar-multiplication syscall and measure guest/circuit tradeoffs.
5. **Return to compiler register allocation only after coverage is fixed.** A
   two-register register-only cache has a measured ceiling near 1%; a real IR
   and allocator is justified only when most execution remains native and the
   exact planner path is correct.
6. **Fuse planning and witness-event generation** if the corrected conventional
   path still cannot approach five seconds.

Profile experiments, logs, function tables, call trees, artifacts, and the
interpreter audit are under `/tmp/ceno-aot-profile-20260727/`.

### Hardware-counter follow-up

After `perf_event_paranoid` was lowered, the cross-trained control was measured
again with `perf stat` and a 499 Hz userspace-cycle profile. Both runs were
pinned to logical CPU 0 and reused the same ABI-3 artifact. The cycle profile
captured 17,203 samples with no lost samples. Counter totals below cover the
whole `mode=execute` process, while the named hot functions are attributable to
the AOT preflight call tree. Counts were collected with a matched six-event set
and were multiplexed at 83.3% in both configurations.

| Metric | Cross-trained control | Target-trained diagnostic | Change |
| --- | ---: | ---: | ---: |
| AOT preflight | 28.257 s | 20.644 s | -26.94% |
| Host instructions | 345.21B | 229.81B | -33.43% |
| Host cycles | 144.97B | 112.73B | -22.24% |
| IPC | 2.38 | 2.04 | -14.29% |
| Branches | 51.83B | 27.75B | -46.46% |
| Branch misses | 0.847B (1.63%) | 1.020B (3.67%) | +20.39% count |
| L1-I misses | 286.44M | 387.42M | +35.25% |
| iTLB misses | 19.21M | 20.60M | +7.21% |
| Artifact size | 114.20 MB | 197.81 MB | +73.22% |

The cross-trained cycle profile's largest relevant exclusive symbols were
`observe_modeled_step` (9.01%), `step_fetched` (5.86%),
`preview_modeled_chips` (5.30%), `aot_exec_one` (3.62%), register loads and
stores (4.99% combined), secp field inversion (3.65%), and
`record_native_access_side_effects` (2.37%). Generated AOT code accounted for
34.11% of whole-process cycles; `L_dispatch` alone was 1.90%. The secp syscall
tree was 12.15% inclusive. A separate 5.55% `HashMap::insert` sample belonged
to program setup outside preflight and is not an AOT optimization target.

Interpretation:

- Coverage is the first candidate with a measured ceiling above the 15% gate.
  Removing 183.8M dynamic-PC fallback steps eliminated 115.4B host
  instructions and reduced preflight by 7.61 seconds even though the naive
  target-trained artifact had *more* branch, L1-I, and iTLB misses.
- Frontend pressure is real, but hot/cold layout is not the first lever: the
  much larger target-trained image ran faster because it avoided Rust exact
  execution and planner work. Coverage must be recovered compactly before
  layout work can be evaluated usefully.
- Next-access hashing is too small to pass the gate independently: the native
  side-effect recorder is about 2.4% exclusive in this profile. It remains a
  worthwhile later cleanup or part of a combined candidate.
- Secp handling is large enough in principle, but changing syscall/circuit
  semantics is higher risk than recovering indirect-target coverage and should
  follow it.

The next feasible production candidate is therefore a **correct, compact
indirect-target coverage artifact**, not another bookkeeping micro-optimization:

1. Start from the already validated selective overflow-block replay logic so
   the candidate is compared against the interpreter's exact boundary oracle.
   Since that repair is performance-neutral, evaluate it together with the
   coverage change as one correctness-and-performance candidate.
2. Add a profiling-only dynamic-miss-PC histogram and classify misses as return
   sites, ELF function entries, or other indirect targets. Do not expand code
   yet. This instrumentation is diagnostic and must have zero disabled-build
   overhead.
3. Seed coverage with statically recoverable link-register return sites and ELF
   function entries. Persist only the remaining observed indirect targets in
   versioned metadata, ranked by recovered execution count, and compile the
   smallest set needed for less than 3% fallback.
4. Reject the candidate if artifact growth is not substantially below the
   naive target-trained 83.61 MB increase, if exact boundaries/next-access/
   ShardRAM differ, or if five-run median gain is below 15%.
5. Only after compact coverage passes, profile the corrected baseline again.
   The next ranked choices are secp transition reduction, then hot-code
   compaction/layout, then next-access event batching. Do not retry two-register
   caching or per-memory-instruction block splitting; their measured gains were
   1.2% and -23.6%, respectively.

Raw PMU data and reports are
`/tmp/ceno-aot-hw-cycles-20260727.data`,
`/tmp/ceno-aot-hw-exclusive-20260727.txt`,
`/tmp/ceno-aot-hw-inclusive-20260727.txt`, and
`/tmp/ceno-aot-hw-{control-matched,target}-stat-20260727.txt`.

### Final cross-trained coverage attempts

The dynamic-miss classifier found 1,405 unique initiating PCs. Of these,
1,381 PCs, representing 231,101 entries, were statically identifiable
link-register return sites. This made return-site closure the smallest
plausible static experiment, but the measured recovery was negligible:

| Candidate | Artifact | Target preflight | Dynamic-PC fallback | Result |
| --- | ---: | ---: | ---: | --- |
| Training-reachable return closure | 136.38 MB | 27.332 s | 183,800,476 | Rejected; recovered 7,443 steps |
| All static return roots | >560 MB assembly before link | Not run | Not run | Rejected; unbounded code growth |
| Raw image code pointers plus return closure | >628 MB assembly before link | Not run | Not run | Rejected; unbounded code growth |
| Typed ELF functions plus image-pointer intersection | 182.18 MB | 27.312 s | 183,423,330 | Rejected; recovered only 384,589 steps |

The final typed candidate trained on block `25607900` produced 36,082 blocks
and 289,174 reachable instructions. Its artifact SHA-256 was
`e1fb92cb3710ba529a73991683ce18ad5160886168a4974f530436d16c2571f2`.
It remained 19.77% fallback on block `25580200`, failed the exact-boundary
oracle, and could not approach the 15% speed gate. All candidate source changes
were reverted; no commit was made. Logs are under
`/tmp/ceno-aot-typed-code-pointers.cgsdCV/logs/` and the classifier log is
`/tmp/ceno-aot-dynamic-entry-classified-20260727.log`.

Per the revised experiment contract, cross-trained selective coverage is now
closed. Subsequent execution candidates train and run the artifact on block
`25580200`. This deliberately stops measuring cross-block generalization; the
block hash, instruction/cycle totals, exact boundaries, next-access state, and
ShardRAM output remain mandatory correctness gates.

### Same-block execution floor and secp doubling candidate

The new same-block ABI-3 control reused the artifact trained and executed on
block `25580200`. Five CPU-0-pinned warm times were `20.571264230`,
`20.634957010`, `20.554636303`, `20.646730979`, and `20.500878832` seconds;
the median was `20.571264230 s` (48.363 MHz). Every run reported 1.36%
fallback, 994,896,527 instructions, 3,979,586,112 cycles, and the same
140-entry shard plan. Logs are under
`/tmp/ceno-aot-same-block-baseline-20260727/logs/`.

A temporary semantic-floor build retained native guest arithmetic, control
flow, memory effects, max-step guards, and syscall fallback but removed all
native access tracking, shard costing, cycle updates, and planner updates. Its
three-run warm median was `8.858493030 s` (`8.858493030`, `8.857555222`, and
`8.864568293`). The result is intentionally not proof-correct, but it proves
that bookkeeping-only work cannot reach the five-second target: the current
generated semantic stream plus syscall fallback already exceeds it. The trim
was reverted. Logs and the hardware sample are under
`/tmp/ceno-aot-semantic-floor-20260727/`.

The next production candidate replaced secp256k1 doubling through general
scalar multiplication by two with the library's dedicated `P + P` point-add
path. It changed no syscall ID, trace record, memory effect, circuit, or AOT
ABI and reused the exact control artifact.

- Candidate warm times: `19.308429150`, `19.254681592`, `19.074125829`,
  `19.118213197`, and `19.103641881` seconds; median `19.118213197 s`
  (52.039 MHz).
- Absolute median gain: `1.453051033 s`; elapsed reduction `7.063522%`;
  throughput speedup `7.600313%`.
- Every run preserved the block hash, instruction and cycle totals, fallback
  histogram, and full shard-boundary list.
- Binary SHA-256 and raw logs are under
  `/tmp/ceno-aot-secp-double-add-20260727/`.

Status: **rejected and reverted**. The candidate was a useful one-line
improvement but missed the independent 15% gate, so it was not retained and
ShardRAM replay was not run.

A follow-up per-execution secp doubling cache was rejected after one probe,
without running the full gate. Of 600,576 calls, 300,032 were cache hits and
300,544 were unique; preflight was still `19.499549125 s`, only about 5.2%
below control. The temporary cache and diagnostic output were reverted. The
probe log is `/tmp/ceno-aot-secp-double-cache-20260727/logs/probe.log`.

The final conventional AOT probe extended atomic executed-step/max-step/PC
handling from register-only blocks to statically preguarded exact-memory
blocks. All 36 runnable AOT tests passed (one ignored), and the full block kept
the expected hash, instruction count, cycle count, fallback histogram, and
140-entry boundary list. However, its first warm probe was `20.764352111 s`,
slower than the `20.571264230 s` control median, while the artifact shrank from
197,809,984 to 189,024,064 bytes. It was rejected early and reverted; logs are
under `/tmp/ceno-aot-all-block-status-atomic-20260727/logs/`.

Together with the `8.858 s` invalid semantic floor, this closes the current
per-block AOT path as a route to five seconds. Even deleting every native
planner/access operation cannot meet the target, and removing more status
bookkeeping does not improve the production path. The next implementation
must eliminate the standalone preflight pass by making shard selection and
cross-shard access-event capture part of the witness replay pipeline.

## Roadmap

### 1. Decompose and profile the 20-second critical path

Add controlled AOT modes while preserving guest semantics:

| Mode | Block costs | Exact memory tracking | Shard planning |
| --- | ---: | ---: | ---: |
| Semantic floor | No | No | No |
| Block accounting | Yes | No | No |
| Exact accounting | Yes | Yes | No |
| Production | Yes | Yes | Yes |

Collect warm-run measurements for:

- host cycles and instructions;
- branches and branch misses;
- L1 instruction-cache and iTLB misses;
- time in Rust fallback and time by ecall code;
- exact-access updates, block entries, and shard-boundary checks;
- generated `.text` size and hot-block coverage.

All modes must produce the same block hash. Production mode must preserve exact
shard boundaries. Intermediate modes are diagnostic upper bounds and must not be
reported as valid proving configurations.

Decision gates:

- If semantic execution exceeds five seconds, prioritize code generation.
- If exact accounting dominates, prioritize block summaries and batched access
  tracking.
- If ecalls consume more than 15-20%, optimize syscall transitions.
- If frontend stalls or iTLB misses dominate, prioritize hot/cold compilation.

### 2. Remove production diagnostic overhead

- Disable the per-ecall `BTreeMap` histogram outside profiling builds.
- Use fixed counters when profiling is enabled.
- Remove per-step diagnostic updates that are not required by shard planning.
- Aggregate fallback statistics after execution where possible.

Acceptance criteria:

- Identical block hash and shard plan.
- Warm preflight at or below 18 seconds.
- No performance regression with profiling disabled.

### 3. Add profile-guided hot/cold AOT compilation

- Record block frequencies and transitions during coverage training.
- Fully compile hot blocks covering at least 99% of executed instructions.
- Route cold blocks through compact shared stubs or an interpreter.
- Order hot blocks by observed transitions and use fallthrough for the common
  successor.
- Place fallback and error paths in cold sections.
- Stitch stable hot paths into traces when safe.

Acceptance criteria:

- Hot native text at or below 20-30 MB.
- At least 99% executed native coverage.
- Instruction-cache and iTLB misses reduced by at least 70%.
- Warm preflight at or below 12-14 seconds.

### 4. Cache guest registers across basic blocks

- Lower each hot block into an SSA-like intermediate representation.
- Load live-in guest registers once and retain active values in host registers.
- Write only live-outs at exits, ecalls, fallback, and checkpoints.
- Constant-fold `x0`, immediates, addresses, and known branches.
- Coalesce adjacent RV32 operations.
- Prefer a proven register allocator such as Cranelift or LLVM if maintaining a
  custom allocator becomes complex.

Acceptance criteria:

- Host instructions per guest instruction reduced by at least 40%.
- Semantic-floor execution reaches at least 250 MHz.
- Warm production preflight at or below 8-10 seconds.

### 5. Move planning from instruction granularity to block granularity

Precompute for every compiled block:

- instruction count;
- additive chip-instance and trace-cell costs;
- register-access contribution;
- compact memory-access descriptors;
- maximum shard-budget requirement.

The common runtime path should execute a block, add its aggregate cost once,
apply a compact memory summary, and check the shard limit once. If a block would
cross the limit, replay only that block through the precise path to find the
exact boundary.

Acceptance criteria:

- At least 95% of executed instructions use aggregate block accounting.
- Precise accounting is limited to shard boundaries and unsupported memory
  patterns.
- Identical shard plan and ShardRAM result.
- Warm preflight at or below 6-7 seconds.

### 6. Specialize and batch exact memory accounting

- Generate trained hot versions for stack, heap, and hints address patterns.
- Validate a known region once at block entry instead of for every memory
  instruction.
- Combine contiguous accesses and deduplicate repeated same-word accesses within
  a block.
- Batch latest-access updates and vectorize contiguous updates where semantics
  permit.
- Retain the generic exact path for failed guards and unusual patterns.

Acceptance criteria:

- Memory-accounting host cycles reduced by at least 50%.
- Identical block hash, shard boundaries, and ShardRAM output.
- Warm preflight at or below 5.1 seconds, or at least 195 MHz.

## Expected Cumulative Result

| Stage | Target time | Throughput |
| --- | ---: | ---: |
| Current | 20.3 s | 49 MHz |
| Diagnostics removed | 17-18 s | 55-59 MHz |
| Hot/cold AOT layout | 12-14 s | 71-83 MHz |
| Block register caching | 8-10 s | 100-124 MHz |
| Block-level planning | 6-7 s | 142-166 MHz |
| Memory specialization | <=5.1 s | >=195 MHz |

These are cumulative acceptance targets, not guaranteed additive gains. Each
stage includes every accepted optimization from the preceding stages. A change
should be retained only after a same-block warm-run comparison confirms its
benefit.

### Comparison protocol for every stage

Use one reproducible benchmark contract throughout the progression:

- Use block `25580200` and require the expected block hash.
- Keep the guest binary, input, shard limits, host compiler flags, and CPU
  machine constant unless the stage explicitly changes generated AOT code.
- Load the AOT artifact before starting the timed region; compilation and cache
  loading are not part of warm preflight time.
- Pin the process to the same physical CPU or NUMA node and use the same CPU
  frequency policy.
- Run at least five warm iterations and report median, minimum, maximum, and
  coefficient of variation. Use the median for the milestone decision.
- Report guest instructions and throughput as
  `994,896,527 / elapsed_seconds / 1,000,000` MHz.
- Require identical exit code, public output, shard boundaries, per-shard cycle
  ranges, and ShardRAM results for production-capable changes.
- Record hardware counters and generated artifact size beside wall time so a
  speedup has an attributable mechanism.

Do not accept a stage based on a single best run or a changed workload.

### Current: 20.3 seconds, 49 MHz

This is the reference production configuration with a warm AOT artifact. The
20.3-second time measures only AOT preflight execution, not artifact training or
compilation.

Observed characteristics:

- `994,896,527` guest instructions are processed.
- Approximately 98.64% of instructions execute through compiled coverage.
- Only 1.36% use fallback, so coverage is not the primary fourfold bottleneck.
- Approximately 1.49M ecalls cross into fallback/helper handling.
- Generated native code is large relative to CPU instruction caches.
- Shard planning and exact latest-access accounting remain active throughout
  execution.

The baseline report must preserve the raw AOT fallback breakdown and artifact
identity. It is the comparison point for all later stages.

Exit condition:

- Three independent benchmark invocations reproduce a median between 20 and 21
  seconds with low run-to-run variance.

### Diagnostics removed: 17-18 seconds, 55-59 MHz

This stage removes observability work that is useful during development but is
not needed to execute or plan the workload.

Implementation scope:

- Compile out the per-ecall `BTreeMap` histogram in production mode.
- Replace dynamic maps with fixed counters in profiling mode.
- Avoid formatting or allocating fallback reports during execution.
- Move nonessential fallback classification and aggregation after execution.
- Compile out per-step debug comparisons and assertions from the benchmark
  binary.
- Keep the counters required for correctness, shard planning, and final summary.

Measurement requirements:

- Measure total time in fallback helpers before and after the change.
- Report ecall count to prove that the workload did not change.
- Report allocator calls from the preflight hot path; the production target is
  zero allocations caused solely by diagnostic collection.
- Confirm that disabling metrics does not alter block or shard output.

The 17-18-second target is a gate, not an assumption. If diagnostic removal
saves substantially less, retain the low-risk cleanup but attribute the
remaining time to code execution and planner accounting.

Exit condition:

- Median warm time at or below 18 seconds with identical outputs and shard plan.

### Hot/cold AOT layout: 12-14 seconds, 71-83 MHz

This stage reduces CPU frontend pressure caused by tens of thousands of compiled
blocks and a native artifact much larger than the instruction cache.

Implementation scope:

- Extend coverage training to collect block-entry counts and transition counts.
- Sort compiled blocks by execution frequency rather than guest address.
- Place mutually frequent blocks next to each other and make the common edge a
  fallthrough.
- Compile hot blocks covering at least 99% of executed instructions with the
  expanded fast path.
- Lower cold blocks to compact shared opcode helpers or the interpreter.
- Move memory-guard, ecall, trap, and error paths into cold text sections.
- Emit symbols or a perf map so samples can be attributed to generated blocks.

Measurement requirements:

- Compare hot `.text`, total `.text`, and loaded executable pages.
- Measure L1 instruction-cache misses, iTLB misses, frontend-stall cycles,
  branches, and branch misses.
- Report hot compiled coverage separately from static program coverage.
- Confirm that cold fallback does not materially increase executed fallback
  steps.

Expected mechanism:

- Fewer executable pages reduce iTLB pressure.
- Frequency layout keeps common transitions within nearby cache lines.
- Cold outlining prevents rare guards and errors from evicting useful code.

Exit conditions:

- Hot native text at or below 20-30 MB.
- At least 99% executed native coverage.
- Instruction-cache and iTLB misses reduced by at least 70% from baseline.
- Median warm time at or below 14 seconds.

If frontend counters are already small at baseline, this stage should be
deprioritized rather than forcing the artifact-size target.

### Block register caching: 8-10 seconds, 100-124 MHz

This stage reduces host instructions spent repeatedly loading and storing the 32
guest registers through `AotRuntimeContext`.

Implementation scope:

- Decode each hot guest block into a minimal intermediate representation.
- Compute register use/definition sets and live-in/live-out registers.
- Keep frequently used guest values in x86 registers across the block.
- Spill only when host register pressure requires it.
- Write guest state at externally visible boundaries: block exits requiring
  dispatch, ecalls, fallback, traps, and shard checkpoints.
- Fold `x0`, immediates, constant addresses, redundant extensions, and known
  branch targets.
- Chain compatible hot blocks into traces so register values can survive common
  block edges.

Correctness requirements:

- Every slow exit must materialize the exact architectural guest-register state
  expected by the interpreter and ecall handlers.
- Differential tests must compare state after every block for representative
  arithmetic, memory, branch, ecall, and fallback cases.
- Full block `25580200` output and shard boundaries must remain identical.

Measurement requirements:

- Report retired host instructions per guest instruction.
- Report loads and stores to the guest-register array.
- Measure spill count and the percentage of hot transitions retaining cached
  registers.
- Measure the diagnostic semantic-floor mode separately from full planning.

Exit conditions:

- At least 40% fewer host instructions per guest instruction.
- Semantic-floor execution at or above 250 MHz.
- Full warm preflight at or below 10 seconds.

If a custom register allocator becomes complex or produces excessive spills,
lower hot blocks through Cranelift or LLVM instead of extending ad hoc assembly
allocation indefinitely.

### Block-level planning: 6-7 seconds, 142-166 MHz

This stage changes planner accounting frequency from once per guest instruction
to once per basic block on the common path.

Implementation scope:

- Precompute additive instruction, chip, trace-cell, and static register-access
  contributions for each compiled block.
- Store compact descriptors for dynamic memory accesses.
- At block entry, verify that the block fits within the remaining shard budget.
- Execute the block and commit aggregate counters once when it fits.
- When it may cross a boundary, execute only that block through precise
  instruction accounting, split at the exact instruction, then resume the fast
  path in the next shard.
- Preserve exact latest-access semantics for addresses whose contribution is not
  safely additive.

Measurement requirements:

- Count fast aggregate blocks, precise blocks, and precise guest instructions.
- Report planner counter updates per guest instruction.
- Measure time in budget checks, cost updates, and boundary recovery separately.
- Compare every shard boundary and cost total with the baseline planner.

Expected mechanism:

- Long blocks pay one budget check and one aggregate update instead of repeated
  per-step updates.
- Exact instruction accounting is concentrated near shard boundaries and
  genuinely dynamic memory behavior.

Exit conditions:

- At least 95% of guest instructions use aggregate block accounting.
- Exact boundary recovery produces the same shard plan.
- Median warm time at or below seven seconds.

### Memory specialization: at most 5.1 seconds, at least 195 MHz

This final conventional AOT stage reduces the remaining cost of exact dynamic
memory and ShardRAM latest-access accounting.

Implementation scope:

- Record address-region profiles for hot memory blocks during training.
- Generate specialized stack, heap, and hints versions where the observed region
  is stable.
- Guard a region once at block entry instead of checking all regions for every
  load and store.
- Collapse repeated accesses to the same word when only the final latest-access
  state is relevant and intermediate accesses do not affect another constraint.
- Batch contiguous latest-access updates and use vectorized host operations where
  this preserves exact semantics.
- Retain the generic exact path for failed guards, mixed regions, MMIO, and
  unusual accesses.

Correctness requirements:

- Specialization guards must fail closed to the exact generic implementation.
- Tests must cover unaligned accesses, boundary addresses, cross-region blocks,
  repeated same-word accesses, shard splits within a block, and failed guards.
- Block `25580200` must preserve exact ShardRAM records and shard boundaries.

Measurement requirements:

- Report specialized-block hit rate and guard-failure rate.
- Report latest-access updates before and after batching.
- Measure host cycles in range checks, latest-access lookup, and update logic.
- Confirm that reduced common-path accounting is not offset by increased code
  size or instruction-cache misses.

Exit conditions:

- Memory-accounting host cycles reduced by at least 50% from the preceding
  stage.
- Median warm time at or below 5.1 seconds.
- Throughput at or above 195 MHz with identical output and shard plan.

If this stage reaches only 6-7 seconds, further per-instruction optimization is
unlikely to reach one second. Continue with the single-pass architecture below
instead of adding increasingly complex special cases.

## One-Second Stretch Architecture

Reaching approximately one second requires removing preflight as a separate
whole-program pass. The preferred pipeline is:

```text
AOT execution and compact event generation
    -> online shard boundary
    -> completed shard checkpoint
    -> GPU trace expansion and proving
```

The AOT executor should emit compact register, memory, syscall, ShardRAM, and
chip-multiplicity events directly. Completed shards should be dispatched to GPU
workers while execution continues. This fuses planning with witness generation
and removes interpreter-backed full witness replay.

An alternative is optimistic fixed-size sharding: use conservative limits,
generate witnesses online, and precisely replay only the final block when a
shard exceeds a trace limit. Slightly less efficient packing is acceptable if it
removes the serial preflight barrier.

Input-specific cached shard plans must not be used to claim end-to-end proving
performance, because witness-generation time is part of the benchmark.

## Proposed Change Sequence

1. `aot: add phase ablations and hardware-counter profiling`
2. `aot: remove production fallback histogram overhead`
3. `aot: add profile-guided hot/cold block compilation`
4. `aot: cache guest registers across basic blocks`
5. `aot: aggregate shard costs at basic-block granularity`
6. `aot: specialize and batch exact memory accounting`
7. `aot: emit streaming witness events and remove separate preflight`

The first delivery objective is a correct warm preflight at or below five
seconds without changing the guest instruction count. The one-second objective
begins only after the standalone planning pass is removed from the critical
path.

## 2026-07-28 co-design gate result

The integrated fixed-capacity tape, FullTracer cursor annotations, direct CPU
witness consumption, dense-memory native path, and block-atomic static-register
bookkeeping candidate was accepted. Its five-run warm combined median is
`19.779601292 s`, versus `24.319528475 s` for the control: an `18.668%`
improvement, clearing the required `15%` ceiling (`20.671599204 s`) by
`0.891997912 s`.

The accepted preflight median is `17.659601292 s`; shard-0 replay and witness
assignment median is `2.11 s`. Every measured run preserved the canonical
block hash, `994,896,527` instructions, `3,979,586,112` cycles, fixed tape
capacity with zero overflow, zero normal-path access callbacks, 136-byte
`StepRecord`, and a verified shard-0 proof. Production validation and the gate
use only `CENO_GPU_WITGEN=0`; GPU witness generation is outside scope.

Three subsequent memory aggregation/guard-removal iterations were profiled and
proved instead of immediately discarding the preliminary result. They measured
`17.899 s`, `17.754 s`, and `18.574 s` preflight respectively, so tracked source
was returned to the accepted ABI-6 checkpoint. The large-block design exposed a
545-access block and quadratic duplicate comparisons; the narrowed and
prevalidated-address variants remained neutral or slower. Detailed
distributions, commits, and retained logs are in
[aot-codesign.md](aot-codesign.md).

## 2026-07-28 CPU-focused priority

Phase-accurate, non-multiplexed counters on the accepted ABI-6 shape measured
global IPC `1.99`, frontend-idle cycles `29.42%`, branch misses `4.60%`, dTLB
misses `2.40%`, and iTLB misses `42.01%`. Generated code accounted for an
estimated `75.09%` of cycles at approximately `1.51` local IPC. This makes
profile-guided hot/cold block layout and common-edge fallthrough chaining the
next AOT code-generation priority. Outline cold guards, traps, and fallback
paths; do not retry the rejected quadratic memory-collapse or
prevalidated-address scratch designs.

Before changing layout, the largest callback inefficiency was removed:
`SECP256K1_DOUBLE` (`600,576` calls) now uses direct point addition instead of
general scalar multiplication by two. Checkpoint `f996f82b` produced a
five-run combined median of `18.018778610 s` (`15.949864257 s` preflight and
`2.06 s` shard-0 witness). This is `8.902%` faster than accepted ABI-6 and
`25.908%` faster than control, with all canonical invariants and shard-0 proof
verification preserved. Detailed counters, local-IPC interpretation, rejected
counter grouping, and artifact paths are in [aot-codesign.md](aot-codesign.md).

## 2026-07-31 frontend attempt, failure analysis, and revised remainder

The frontend priority was tested in two stages against a fresh isolated ABI-6
control from Ceno `6ab15eb0`, always on block `25580200`, CPU 0,
`CENO_GPU_WITGEN=0`, `jemalloc,gpu,aot`, and a `268435456`-cell shard limit.
The control five-run warm preflight median was `16.250643596 s`; promotion
required at most `15.438111416 s` and exact proof semantics.

First, ABI 7 kept the whole image but used dense block/edge counts,
deterministic weighted chains, and common-successor fallthrough. It achieved
`65.49%` weighted fallthrough coverage and a `16.212812992 s` median, only
`0.233%` faster than control. Its artifact remained effectively unchanged
(`191,101,064` versus `191,207,560` bytes). It was correct through shard-0
proof verification but missed the 5% performance gate, so whole-image chaining
is rejected as a promotion candidate.

Second, ABI 8 retained full bodies for 5,550 PC-ordered blocks covering
`99.500%` of trained native instructions and routed all other PCs through
shared typed fallback stubs. This reduced the artifact from `191,207,560` to
`64,861,848` bytes, `.text` from `179,959,034` to `62,546,297` bytes, and RX
pages from 43,936 to 15,271. FullTracer reused the identical persisted hot set
and layout.

The compact candidate nevertheless regressed: its five-run median was
`16.431885731 s` (`+1.115%`). Fallback rose from 2,036,036 (`0.20%`) to
6,999,492 (`0.70%`), adding 4,963,456 assembly-to-Rust recovery steps. One
shard boundary moved by 20 cycles (`770,893,916` to `770,893,936`) because a
cold block switched from block-atomic native accounting to per-step planner
recovery. Cold ECALL recovery also polluted the diagnostic histogram with stale
reason codes. The final hash, instruction/cycle totals, tape usage, and overflow
state remained stable, but exact shard planning did not.

Phase-controlled, non-multiplexed AOT-only counters explain the latency result:

| Event | ABI 6 | ABI 8 | Change |
| --- | ---: | ---: | ---: |
| Host cycles | 71,669,404,678 | 72,015,887,986 | +0.48% |
| Host instructions | 133,094,394,948 | 136,689,899,107 | +2.70% |
| Frontend-idle cycles | 23,157,546,212 | 22,197,566,151 | -4.15% |
| Branches | 17,853,602,231 | 18,524,188,783 | +3.76% |
| Branch misses | 854,584,620 | 836,738,903 | -2.09% |
| L1-I misses | 369,327,258 | 308,579,377 | -16.45% |
| iTLB misses | 24,401,010 | 19,704,179 | -19.25% |

The intended mechanism therefore worked: frontend idle, L1-I misses, and iTLB
misses all decreased. It did not work as an optimization because total mapped
pages were not the active working set, while fallback recovery increased real
retired work. Four planner/interpreter recovery symbols each cross the 0.5%
threshold and together account for at least 2.28% of candidate cycles. Across the
complete reports, named recovery rises from 1.18% to 2.84%, and dispatch grows
from 1.30% to 1.47%. The saved frontend cycles were smaller than the recovery
cost.

The remaining control samples are distributed across generated memory regions
(`33.96%`), block accounting (`15.41%`), guest bodies (`10.31%`), and dispatch
(`1.30%`). This is enough to prioritize the controlled ablations below, but not
enough to justify another speculative code change: memory and accounting must
first be separated without changing guest execution.

Status: **ABI 7 and ABI 8 rejected; ABI-8 compaction reverted.** The footprint
mechanism passed, but the latency and exact-boundary gates failed. Per the
staged stop rule, do not proceed to hot-section chaining, additional cold
outlining, or huge pages. The reverted tree passes all AOT tests (`58` passed,
one ignored, plus two integration tests) and the `ceno_zkvm` AOT feature check.

### Revised remaining optimization sequence

The old 12-14 second hot/cold target is not supported by the measured result.
The next work must attack host instruction count while preserving native
block-atomic planning:

1. Use ABI 6 as the matched baseline; keep ABI-7 dense frequency/edge data only
   as profiling infrastructure until a candidate clears the gate.
2. Add or run the four controlled modes already specified in Roadmap step 1:
   semantic floor, block accounting, exact memory without shard planning, and
   production. Collect the same core, branch, L1-I, and iTLB groups.
3. Rank the instruction delta between those modes. Start one surgical candidate
   only if the attributable component exceeds 5% of total AOT cycles.
4. Prefer reducing generated hot-block loads/stores, dispatches, or planner
   updates without Rust transitions. Do not retry the rejected broad
   two-register cache, quadratic duplicate search, prevalidated-address scratch
   cache, or selective cold fallback.
5. Any future compaction must keep native block accounting for every trained
   block that can affect a shard boundary; a generic per-step fallback is not a
   semantics-preserving replacement.

Raw data: `.codex-results/aot-hot-layout-20260731/`,
`.codex-results/aot-hotcold-20260731/`, and the matched profiles in
`.codex-results/aot-hotcold-20260731/profile/`. Full analysis is mirrored in
[aot-codesign.md](aot-codesign.md).

## 2026-07-31 matched cost ablations

The four controlled modes requested above have now been measured. A temporary
profiling binary fixed an execute-mode harness defect: unlike witness mode,
execute had reused an AOT context prepared without the configured
`MultiProver`, producing an invalid `u64::MAX` cache key. The corrected binary
explicitly prepared preflight AOT with the real cell/cycle limits. The
production source, local GPU path patch, and `Cargo.lock` were restored after
the build; the diagnostic selector exists only in the copied binary.

All latency runs used block `25580200`, cached `--chain-id 1` input,
`jemalloc,gpu,aot`, `CUDA Backend Enabled`, CPU 0, `CENO_GPU_WITGEN=0`, and
`CENO_MAX_CELL_PER_SHARD=268435456`. Each row is the median of five cache-hit
warm runs:

| Mode | Warm median | Change from control | AOT artifact |
| --- | ---: | ---: | ---: |
| ABI-7 control | `16.215944679 s` | control | `191,101,064 B` |
| no shard-cost accounting | `13.785318122 s` | `-14.989%` (`-2.431 s`) | `173,570,184 B` |
| no exact access/tape maintenance | `10.316278991 s` | `-36.382%` (`-5.900 s`) | `117,336,200 B` |
| semantic execution floor | `7.764178268 s` | `-52.120%` (`-8.452 s`) | `93,452,424 B` |

The no-access trim is the dominant direction. It kept native guest execution,
block cost accounting, `994,896,527` executed AOT steps, `3,979,586,112`
guest cycles, 0.20% fallback, and the 657 capped-run planner boundaries, while
reducing tape usage from `25,334,834` to `581,366`. It is intentionally not a
correctness candidate because those missing access events are required by
FullTracer/witness replay. The no-accounting trim is also diagnostic only: it
changed the capped run from 657 to 394 shards. The semantic-floor report counts
only fallback steps in `PreflightTracer`, so its timing is a lower bound, not a
proof-valid execution result.

The remote shard-count setting is `CENO_MAX_CELL_PER_SHARD=4500000000`.
ABI-7 AOT produced 35 shards, preserved `994,896,527` instructions and
`3,979,586,112` cycles, used `16,809,729 / 18,911,061` tape events with zero
overflow, and retained 0.20% fallback. Its cold target span was
`16.100995288 s`; compilation/training was excluded. AOT uses basic-block
atomic planning, so equality with interpreter boundary positions is no longer
a gate. Correctness instead requires AOT preflight, FullTracer replay, witness
generation, and proof verification to reuse AOT's own identical 35-boundary
plan.

### Revised path to preflight plus shard 0 below five seconds

Instrumentation-only work cannot meet the target. The semantic floor is
`7.764 s`; adding the approximately `2.04 s` shard-0 replay/witness span gives
an optimistic floor near `9.8 s`. Use this sequence:

1. Keep the retained secp256k1-double fix and use AOT's block-atomic plan as
   the AOT correctness baseline. At the 4.5B remote setting, require the same
   35 boundaries across AOT preflight, FullTracer replay, witness generation,
   cache reloads, and proof verification; do not require interpreter boundary
   equality.
2. Split the 5.9-second access delta into static-register and dynamic-memory
   trims. Implement only the larger half first. Prefer block summaries,
   epoch-tagged latest-access entries, and direct batched tape writes; preserve
   exact event order and avoid Rust transitions.
3. Make shard accounting update cost buckets only when a chip count crosses a
   power-of-two boundary. Preserve per-block accept/split decisions and exact
   rollback semantics. The measured upper bound for this layer is 2.43--2.55
   seconds.
4. Re-measure the faithful combined candidate. A general-input result around
   8--10 seconds is plausible; below five seconds is not supported by the
   current native semantic floor.
5. For the cached-block benchmark, prototype an exact-input preflight-result
   cache keyed by program digest, complete input/hints digest, cost-model ABI,
   cell/cycle limits, and AOT layout digest. Persist shard boundaries and the
   exact next-access tape, validate all keys before reuse, and fall back to a
   full preflight on any cache-key mismatch. This removes the whole 994M-step
   warm preflight and is the only measured path with enough headroom for
   preflight-plus-shard-0 below five seconds.
6. If exact-input caching is out of scope, reaching five seconds requires a new
   guest-code backend (profile-selected block SSA/register allocation), not
   another layout tweak. Do not retry the rejected two-register cache,
   quadratic memory aggregation, scratch-address cache, cold fallback, or huge
   pages.

Raw logs and warm samples are under
`.codex-results/aot-ablation-20260731-v2/`. The copied diagnostic binary is
`.codex-sanity-run/aot-ablation-20260731/profile-bin-v2` with SHA-256
`771ff25a3102c1366ab91a4b9a2f71d651c58b33799a6aeefdd03f6db6b818b2`.

## 2026-07-31 native tape/access checkpoint

ABI-9 removes the block-static Rust access helper and keeps the next-access
cursor resident in `%rbx`. The executed-step output pointer moved to the spare
stack slot; native cursor state is flushed before fallback, synchronization,
shard split, callbacks, halt, and errors, then reloaded after callbacks. Static
block events are appended directly and release builds batch first-touch count
updates. Generic and FullTracer entry ABIs are unchanged.

On cached mainnet block `25580200`, pinned to CPU 0 with
`CENO_MAX_CELL_PER_SHARD=4500000000`, the retained stage produced warm samples
`16.392697`, `16.031515`, `16.071714`, `16.136394`, and `16.064960` seconds;
median `16.071714 s`. The ABI-7 control median was `16.215945 s`, so this is a
`0.889%` reduction. It is retained as a native-only cleanup per the explicit
design preference despite not meeting the original 5% timing threshold.

Correctness remained exact: block hash
`34439c597563024690ce3c91a082c34507569c7e18cc4d1b3b68550b791a2773`,
`994,896,527` instructions, `3,979,586,112` cycles, `16,809,729` tape events,
zero overflow, zero normal access-helper calls, 0.20% fallback, and the same
35 AOT boundaries. The generated preflight artifact shrank from `191,101,064`
to `186,124,424` bytes (`-2.60%`); FullTracer replay grew from `146,568,120`
to `150,528,952` bytes (`+2.70%`). Raw logs are under
`.codex-results/aot-native-tape-access-20260731/candidate/`.

The first sparse-accounting implementation was rejected. Its warm samples
were `17.303190`, `17.533018`, `17.343788`, `17.480791`, and `17.309448`
seconds; median `17.343788 s`, a `6.96%` regression. It preserved the same
tape, totals, fallback, and 35 boundaries, but per-chip threshold-state traffic
cost more than the bucket work it skipped. Logs are under
`.codex-results/aot-native-tape-sparse-20260731/candidate/`.

## 2026-07-31 block-capacity checkpoint

ABI-10 hoists event-tape capacity checking to one conservative guard per
native block. The guard reserves the maximum possible register/memory event
count for that block; qualifying appends are then straight native stores and a
resident-cursor increment. Five warm samples were `14.279486`, `14.427257`,
`15.202264`, `15.797175`, and `15.454057` seconds, median `15.202264 s`. This
is `-5.410%` from ABI-9 and `-6.251%` from the ABI-7 control.

The cold and all warm runs retained the expected hash, `994,896,527`
instructions, `3,979,586,112` cycles, `16,809,729 / 18,911,061` tape usage,
zero overflow/access helpers, 0.20% fallback, and the same 35 boundaries. The
preflight artifact is `169,187,464` bytes, `-11.47%` from ABI-7. A shard-0
witness correctness pass completed with the same plan; FullTracer positioning
took `1.85 s`, replayed `26,628,726` steps, and used `396,947` Rust fallback
transitions. Full witness assignment took `43.5 s` and is outside the replay
target span. Logs are under `.codex-results/aot-block-capacity-20260731/candidate/`.

A subsequent `prove-app --shard-id 0` attempt reproduced the same shard-0
FullTracer replay (`26,628,726` steps, `396,947` Rust transitions), but the
prover then positioned shard 1 for continuation context and was terminated by
SIGKILL after `118.59 s`, before proof output or verification. Thus shard-0
witness generation is verified, but the proof-verification gate remains
incomplete on this host. The attempt is recorded in `shard0-proof.log` and
`shard0-proof.time` in the same artifact directory.

Against the `7.764178 s` semantic floor, faithful preflight is still `1.958x`,
so the final 1.10x gate is not met. Preflight plus shard-0 positioning is about
`17.05 s`, also above the combined target.

### Rejected same-bucket cost fast path

ABI-11 tested a lighter sparse-accounting scheme without threshold arrays. It
still updated every chip count, but branched around the trace/main/tower delta
work when the old and candidate padded buckets were equal. Five uncontended
warm samples were `15.480995`, `15.391056`, `15.282137`, `15.228894`, and
`15.187410` seconds, median `15.282137 s`. That is `0.525%` slower than the
ABI-10 checkpoint and therefore fails the 5% retention gate.

The cold pass and every retained sample preserved the expected hash,
`994,896,527` instructions, `3,979,586,112` cycles, exact tape usage, zero
overflow/access callbacks, 0.20% fallback, and all 35 boundaries. Its artifact
was `169,359,496` bytes (`+0.10%` from ABI-10). The code was reverted to ABI-10.
Two concurrently launched measurements (`warm2` and `warm3`) are present in
the directory but excluded from the median. Raw logs are under
`.codex-results/aot-sparse-bucket-skip-20260731/candidate/`.

### Rejected first-touch accumulator

ABI-12 batched release-build first-touch increments in a native stack slot and
published the total with the resident tape cursor at Rust-visible boundaries.
It also omitted release-only event-address context stores after removal of the
access callback. Five warm samples were `15.560373`, `15.651944`, `15.472060`,
`15.547530`, and `15.540207` seconds, median `15.547530 s`; this regressed the
ABI-10 checkpoint by `2.271%` and was reverted.

The cold pass and warm runs preserved the expected hash, instruction/cycle
totals, exact tape and first-touch totals, zero overflow/access callbacks,
0.20% fallback, and all 35 boundaries. The artifact was `173,324,424` bytes
(`+2.45%` from ABI-10). A register-resident precursor was rejected before
measurement when the debug callback test exposed an assembly-local-label
collision; the collision was fixed and the complete suite then passed before
the stack candidate was measured. Raw logs are under
`.codex-results/aot-first-touch-batch-20260731/candidate/`.
