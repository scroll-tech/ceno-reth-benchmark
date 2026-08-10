# AOT Preflight Tracking Overhead

## Scope

This document compares two Ceno AOT execution modes on Ethereum block `25580200`:

- **Pure AOT:** executes guest-visible values and architectural state without trace or access bookkeeping.
- **Tracked Preflight AOT:** executes the same guest program while constructing shard-aware access history and planner state.

Current representative results:

| Mode | Elapsed time |
|---|---:|
| Pure AOT median | ~2.37s |
| Tracked Preflight total | ~8.24s |
| Tracked native execution | ~6.59s |
| Tracked Rust fallback/syscalls | ~1.63s |
| Total tracked-mode gap | ~5.87s |

The tracked run preserves the exact output hash, `994,896,527` guest instructions, `16,865,461` next-access events, and 35 shard boundaries.

## Execution flow

```text
Dispatch
  -> block-entry guards
  -> shard-cost admission
  -> register first-access tracking
  -> guest instructions
       -> compute/control flow
       -> memory value + packed stamp + access event
  -> register latest-access commit
  -> cycle and planner commit
  -> next block or synchronized Rust callback
```

Pure AOT performs the guest execution in this flow. Tracked Preflight adds the guards, access relations, stamps, events, planner transitions, and synchronization needed for witness generation and exact sharding.

## 1. Memory execution and access tracking

Generated-code profile share: **47.0%**, or roughly **3.10s** of the 6.59s native interval. This includes both useful guest memory execution and tracking overhead.

### Pure AOT

- Computes and validates the guest address.
- Loads or modifies the 32-bit guest value.
- Writes store values immediately.
- May validate all memory accesses once at block entry.
- Does not update the packed latest-access stamp.

### Additional tracked work

- Classifies accesses as heap, stack, hints, or ordinary dense memory.
- Updates memory-region minimum and maximum addresses.
- Loads the full packed value/stamp cell.
- Decodes the previous-access cycle.
- Checks whether the previous access belongs to another shard.
- Appends an exact `NextAccessEvent` when required.
- Computes and writes a new packed stamp after every load and store.

### Measured causal overhead

- Removing access/event bookkeeping saved about **0.55s native**.
- Removing packed-stamp traffic as well raised the combined saving to about **0.99s native**.

### Optimization direction

- Measure repeated-address coverage first.
- Use a small region-local stamp cache when repetition is sufficient.
- Read the previous stamp once and commit the final stamp once.
- Preserve store values immediately.
- Flush before callbacks, aliases, exceptional accesses, or shard splits.
- Consider bulk event materialization only if exact ordering remains unchanged.

## 2. Shard planner accounting

Generated-code profile share: **16.9%**, or roughly **1.11s**.

### Pure AOT

- Tracks only the instruction budget and total executed instructions.
- Reserves the complete instruction count once for eligible blocks.
- Does not calculate chip costs or shard admission.

### Additional tracked work

- Loads each block's chip-contribution descriptor.
- Updates per-chip instance counts.
- Detects logarithmic cost-bucket transitions.
- Updates trace, main, and tower cost estimates.
- Checks cell and cycle limits.
- Splits before a block that does not fit and reinitializes it in the next shard.

### Measured causal overhead

- Invalidly removing planner work reduced native time by about **1.60s**, but changed shard boundaries.
- The retained exact bucket-ceiling cache recovered about **0.49s**.
- Approximately **94.8%** of planned blocks remain inside their current cost buckets and now skip cost-table recomputation.

### Optimization direction

- Aggregate chip contributions across hot multi-block regions.
- Prove that every relevant prefix remains below shard limits.
- Update counts and costs once per safe region.
- Use the exact block path near a possible boundary.
- Roll back only the unexecuted suffix after an unexpected conditional edge.

## 3. Entry, exit, and memory guards

Generated-code profile share: **13.6%**, or roughly **0.90s**.

### Pure AOT

- Checks the remaining instruction budget.
- Checks memory alignment and dense-memory membership.
- Detects unsupported or exceptional operations.
- Hoists eligible memory validation to block entry.

### Additional tracked work

- Checks event-tape capacity.
- Performs exact heap, stack, and hints classification.
- Maintains memory extrema.
- Enforces block atomicity before deferring register state.
- Ensures deferred state is flushed before externally visible boundaries.

### Optimization direction

- Reserve instruction and event capacity once per region.
- Group memory operations by base register.
- Validate `[base + minimum offset, base + maximum offset]` once when safe.
- Update region extrema once.
- Retain the exact guard path whenever the proof fails.

A previous block-only memory fusion saved about **64ms**, so a meaningful gain likely requires region-level amortization.

## 4. Register access tracking

Generated-code profile share: **6.0%**, or roughly **0.40s**.

### Pure AOT

- Reads `rs1` and `rs2` for instruction semantics.
- Writes `rd` and preserves architectural `x0` behavior.
- Does not record access cycles.

### Additional tracked work

- Finds each register's first access in the block.
- Checks a shard-local touched mask.
- Appends a previous-to-current event for a shard first touch.
- Commits each register's latest access cycle at block exit.
- Resets shard-local state after a split.

### Optimization direction

- Union register accesses across a fused region rather than one block.
- Perform one first-touch check and one latest commit per register per region.
- Keep register values resident inside the region.
- Flush before callbacks, shard splits, and unexpected edges.

## 5. Cycle and planner-state commit

Explicit plan-commit profile share: **1.2%**, or roughly **0.08s**. Some related work is attributed to adjacent regions.

### Pure AOT

- Updates the executed-instruction count, usually once per eligible block.

### Additional tracked work

- Publishes the global tracer cycle and pending native steps.
- Updates current-shard cycle and step counts.
- Publishes PC and trace state required by callbacks.

### Optimization direction

- Accumulate these values across a fused region.
- Publish once at region exit.
- Flush immediately before callbacks, shard splits, and exceptional exits.

The direct saving is small, but this is required infrastructure for broader region fusion.

## 6. Dispatch and control flow

Generated-code profile share: **2.6%**, or roughly **0.17s**.

### Pure AOT

- Executes branches and direct fallthroughs.
- Uses the dispatch tree for non-adjacent or indirect targets.
- Returns to Rust for unsupported behavior.

### Additional tracked work

- Synchronizes the event cursor, pending cycles, access state, and planner state before leaving generated execution.
- Reloads state after Rust may have changed the planner or shard.
- Executes from larger generated blocks, increasing instruction-cache pressure.

### Optimization direction

- Fuse hot adjacent and conditional edges.
- Keep the trained edge as native fallthrough.
- Route cold edges through one exact synchronization exit.
- Keep dispatch below planner, memory, and guard work in priority.

## 7. Rust fallback and syscalls

Measured separately from native time: approximately **1.63s**.

### Pure AOT

- Executes supported value-only syscall kernels.
- Updates guest registers and memory.
- Does not emit access history or planner observations.

### Additional tracked work

- Records the `ECALL` fetch and argument-register accesses.
- Applies the syscall's register and memory access plan.
- Updates packed stamps and latest-access tables.
- Appends events in exact order.
- Updates chip contributions and may split a shard.
- Flushes generated state before Rust and reloads it afterward.

### Optimization direction

- Batch known syscall access plans.
- Reduce synchronization around direct tracked syscalls.
- Update multiple stamps and events in one native helper.
- Avoid invalidating planner caches for helpers that provably cannot mutate planner state.

Direct tracked syscall handling has already removed about **1.54s**. The remaining time includes unavoidable cryptographic computation as well as tracking overhead.

## Priority

The highest-value combined design is:

1. Form hot conditional and memory-safe native regions.
2. Aggregate exact planner accounting across each proven-safe region.
3. Reserve instruction and event guards once per region.
4. Union register first/latest accesses across the region.
5. Add a local memory-stamp cache only when measured address repetition justifies it.

Small pointer-residency and dispatch-only experiments have not produced material improvements. The remaining opportunity is reducing how often exact tracking work runs, while retaining the original path at every boundary where a split, callback, alias, or exceptional access may occur.

## Measurement artifacts

- Latest generated-code profile: `.codex-results/aot-below5-20260809/planner-bucket-abi54/perf.data`
- Profile report: `.codex-results/aot-below5-20260809/planner-bucket-abi54/perf-report.txt`
- Current ABI 56 results: `.codex-results/aot-below5-20260809/planner-bucket-abi56/`
- Full campaign history: `../ceno/aot-plan.md`

## Budgeted Incremental Preflight AOT Tracking (2026-08-10)

This campaign rebuilds tracked Preflight cumulatively from the current
approximately 2.374s Pure native path. Each stage is a separately cached AOT
artifact selected with `--mode aot-tracking --aot-tracking-stage <stage>`.
Disabled responsibilities must be absent from generated code. Full must use
the production emitter and callbacks, and production switches to that path only
after exact parity is established.

Runtime budget:

```text
Pure median                         ~2.374s
Maximum Full Preflight median        5.000s
Available tracking budget           ~2.626s
Planned tracking allocation          2.350s
Contingency reserve                  0.276s
```

### Live stage and budget ledger

| Stage | Newly added work | Status | Revision / cache identity | Expected work | Observed work | Raw samples / median | Marginal | Cumulative | Allocated | Consumed | Remaining total budget | Correctness / artifacts |
|---|---|---|---|---|---|---|---:|---:|---:|---:|---:|---|
| `Pure` | Existing value-only execution | provisional smoke passed; canonical median pending | `f73dc3f4` / `a38e…-abi59-pure-x86_64-linux-cells268435456-cycles536870912` | Value-only execution | Value-only staged callback and native body | `2.441866106s` / provisional single sample | — | `0.000000000s` | — | `0.000000000s` | `2.626000000s` | exact hash, exit 0, 994,896,527 instructions; `.codex-results/aot-tracking-20260810/pure-abi59-smoke.log` |
| `Runtime` | Container, callback/syscall routing, synchronization shell | provisional smoke passed | `f73dc3f4` / `a38e…-abi59-runtime-x86_64-linux-cells268435456-cycles536870912` | Exact shell, no later-stage work | Preflight container with pure native execution and stage-safe syscall/fallback callback | `2.428137201s` / provisional single sample | `-0.013728905s` | `-0.013728905s` | `0.200s` | `0s` (negative delta retained as noise/slack) | `2.639728905s` | exact hash, exit 0, 994,896,527 instructions; `.codex-results/aot-tracking-20260810/runtime-abi59-smoke.log` |
| `ExecutionState` | Cycles, PC before/after, instruction kind, step publication | provisional smoke passed; borrowed `0.043275698s` cumulative | `f73dc3f4` / `a38e…-abi59-execution-state-x86_64-linux-cells268435456-cycles536870912` | Exact block-batched execution metadata | Exact cycle `3979586112`, PC transition and ECALL kind; preflight-ABI block body carries next PC resident and commits once | `2.785141804s` / provisional single sample | `+0.357004603s` | `+0.343275698s` | `0.300s` cumulative | `0.343275698s` cumulative | `2.282724302s` | exact hash, exit 0, instruction count; 5 focused tracking tests pass; `.codex-results/aot-tracking-20260810/execution-state-abi59-resident-smoke.log` |
| `Planner` | Chip counts, bucket costs, admission, shard transitions | specialization retained; budget gate still failed, so advancement stopped | `7c79627d` / `a38e…-abi61-planner-x86_64-linux-cost46a252…-cells4500000000-cycles536870912` | Exact planner and 35 shards at 4.5B cells | One/two-chip descriptors embedded and unrolled; larger descriptors retain the generic loop | cold `5.026433376s`, cached `4.825796107s` / provisional single samples | `+2.040654303s` (cached adjacent) | `+2.383930001s` | `0.850s` cumulative | `2.383930001s` cumulative | `0.242069999s` | exact hash/count/cycle, complete 36-point boundary vector and 35 costs/shards; `.codex-results/aot-tracking-20260810/planner-specialized-abi61/` |
| `RegisterLatest` | Register first/latest state without events | retained; budget remains failed | `cb3cc5b9` / `a38e…-abi62-register-latest-x86_64-linux-cost46a252…-cells4500000000-cycles536870912` | Exact latest cycles/touched state | Initialized-register subset guard skips redundant first-touch probes; exact block-last commits remain | cold `5.529363466s`, cached `5.465545408s` / provisional samples | `+0.639749301s` | `+3.023679302s` | 0.200s | `0.639749301s` | `-0.397679302s` | exact hash/count/cycle and complete planner vector; 76 AOT tests pass; `.codex-results/aot-tracking-20260810/register-latest-mask-abi62/` |
| `MemoryLatest` | Packed stamps and latest memory cycles | measured; ABI-63 guard cleanup inherited | `f12cb498` / `a38e…-abi63-memory-latest-x86_64-linux-cost46a252…-cells4500000000-cycles536870912` | Exact packed memory state | Packed `{ordinal,value}` update; block guard proves alignment/dense membership once | cold `5.663222366s` / provisional single sample | `+0.197676958s` | `+3.221356260s` | 0.600s | `0.197676958s` | `-0.595356260s` | exact hash/count/cycle and complete planner vector; `.codex-results/aot-tracking-20260810/memory-latest-abi63/` |
| `MmioBounds` | Heap, stack, hints classification/extrema | retained; budget remains failed | `f12cb498` / `a38e…-abi63-mmio-bounds-x86_64-linux-cost46a252…-cells4500000000-cycles536870912` | Exact extrema | Hoisted guard records two-bit region; body updates only that region without reclassification | cold `7.091111770s`, cached `7.816615748s` / provisional noisy samples | `+1.427889404s` (cold adjacent) | `+4.649245664s` | 0.100s | `1.427889404s` | `-2.023245664s` | exact hash/count/cycle/vector; complete emulator suite and assembly inspection pass; `.codex-results/aot-tracking-20260810/mmio-regions-abi63/` |
| `EventCapacity` | Cursor, guards, growth, synchronization | measured; existing emitter retained | `f12cb498` / `a38e…-abi63-event-capacity-x86_64-linux-cost46a252…-cells4500000000-cycles536870912` | Exact capacity behavior, no events | Per-block capacity guards; trained tape has no growth | cold `7.840320821s` / provisional single sample | `+0.749209051s` | `+5.398454715s` | 0.100s | `0.749209051s` | `-2.772454715s` | exact hash/count/cycle/vector and capacity 18,973,696; ABI-64 trim rejected; `.codex-results/aot-tracking-20260810/event-capacity-abi{63,64}/` |
| `RegisterEvents` | Register next-access events | measured; existing emitter retained | `f12cb498` / `a38e…-abi63-register-events-x86_64-linux-cost46a252…-cells4500000000-cycles536870912` | Golden register-only tape | Exact 24-byte first-touch entries; memory events absent | cold `7.922237605s` / provisional single sample | `+0.081916784s` | `+5.480371499s` | 0.150s | `0.081916784s` | `-2.854371499s` | exact 1,086-event tape and canonical state/vector; disassembly inspected; `.codex-results/aot-tracking-20260810/register-events-abi63/` |
| `Full` | Memory next-access events and complete parity | measured; acceptance failed | `f12cb498` / `a38e…-abi63-full-x86_64-linux-cost46a252…-cells4500000000-cycles536870912` | Exact production Preflight state/tape | 16,865,461-event tape; byte-identical production emitter | cold `7.845179490s` / provisional single sample | `-0.077058115s` | `+5.403313384s` | 0.350s | `0s` (negative delta retained as noise/layout) | `-2.777313384s` | exact hash/count/cycle/vector/tape counts; final `<5.0s` gate failed; `.codex-results/aot-tracking-20260810/full-abi63/` |

Budget borrowing must be recorded in this ledger. Advancement stops if measured
cumulative cost plus remaining minimum allocations projects above 2.626s.
Adjacent unrounded medians must reconcile exactly:

```text
marginal_i   = median(stage_i) - median(stage_i-1)
cumulative_i = median(stage_i) - median(Pure)
sum(marginal_i) = median(Full) - median(Pure)
residual = 0
```

### Architecture and acceptance plan

- Extend block plans with compile-time instruction/cycle totals, register
  first/latest sets, memory base/offset guards, maximum event capacity,
  planner/chip contributions, and entry/exit/exceptional flush descriptors.
- At block entry reserve budgets and event capacity, validate proven stable
  address intervals, apply planner admission/contributions, process static
  register unions, and load resident state. Use the exact path if atomicity is
  not proven.
- Keep only dynamic arithmetic/control flow, load/store values, dynamic address
  and alias resolution, memory stamp transitions, and enabled first-touch work
  in instruction bodies. Commit cycles/steps, register latest cycles, extrema,
  planner state, and event cursor once at block exit.
- Flush before syscalls, fallbacks, exceptions, shard splits, tape relocation,
  indirect exits, and Rust callbacks; reload afterward. Document a stable
  x86-64 convention for context, guest registers, packed memory, event cursor,
  shard start, planner bases, cycle, and pending steps.
- Before accepting each stage, inspect assembly for absent later-stage code,
  once-per-block deterministic work, non-duplicated memory/register/planner
  work, stage-correct capacity checks, resident hot state without spills, and
  absent diagnostic counters.
- Apply every enabled responsibility consistently to native instructions,
  direct syscalls, and fallbacks. Required equality grows cumulatively from
  architectural state (`Pure`/`Runtime`) through execution metadata, planner,
  latest-access state, extrema, capacity behavior, register events, and finally
  the complete production tape and state.
- After each stage run focused emulator tests and cached smoke runs, inspect
  assembly, record total/native/fallback timing and counters, update this
  ledger, and make only a bounded stage-local correction when needed.
- Canonical measurement uses five cached warm runs per stage in alternating
  ascending/descending order, one pinned CPU with governor recorded, and only
  `run_to_halt` timed. Setup, allocation, finalization, and replay are reported
  separately, along with host cycles, instructions, branches, branch misses,
  and cache misses. Both blocks 25580200 and 25687400 are measured. Counter
  artifacts are separate from timing artifacts.
- Optimize completed stages from largest marginal cost downward and rerun the
  entire ladder after each retained optimization.

Final acceptance requires a Full median below 5.0s on block 25580200; hash
`34439c597563024690ce3c91a082c34507569c7e18cc4d1b3b68550b791a2773`;
exactly 994,896,527 instructions and 35 shard boundaries; exact PC, registers,
memory, exit status, tape, extrema, costs, planner state, FullTracer replay, and
witness sanity; no more than 2% Pure or block 25687400 regression; and no public
syscall ABI, AIR, witness-format, or proof-behavior change. Failed or reverted
attempts and all evidence classifications remain recorded in the live ledger.

### Implementation checkpoint

The typed CLI/cache framework and all ten compile-time stage selections are
implemented. ABI 59 identities include the stage and shard cell/cycle layout.
Focused tests prove the identities are distinct and ordered, disabled later
responsibilities are absent from generated assembly, cumulative AOT state
matches the same-stage interpreter, and `Full` emits byte-for-byte the same
assembly as production `PreflightDirectBlockPlan`.

Pure and Runtime pass provisional block-25580200 smokes with the acceptance
hash and 994,896,527 instructions. ExecutionState publishes exact cycle,
PC-before/after, kind and step state; its internal block style preserves the
preflight resident-register/packed-memory ABI, carries next PC in a native
register, and commits state once at block exit. This reduced the stage to
2.785141804s and allowed advancement.

Planner is now the active failed gate. At the same 268M diagnostic layout it
measured 5.359623084s, a +2.574481280s adjacent delta. At the canonical 4.5B
cell layout it measured 5.381945825s and produced 35 predicted shard costs
(35 shards) plus 36 boundary points including the initial cycle, matching the
production convention. Later stages have not been advanced. These are
provisional samples, not canonical medians.

The measured adjacent marginals reconcile exactly for the provisional prefix:

```text
-0.013728905s + 0.357004603s + 2.574481280s = 2.917756978s
5.359623084s - 2.441866106s = 2.917756978s
residual = 0
```

After retained ABI 61 specialization, the current cached provisional prefix is:

```text
-0.013728905s + 0.357004603s + 2.040654303s = 2.383930001s
4.825796107s - 2.441866106s = 2.383930001s
residual = 0
```

Only `0.242069999s` of the `2.626s` tracking budget remains at Planner.
The remaining stages have minimum allocations totaling `1.500s`, so the
projected prefix exceeds the budget by `1.257930001s`. Per the advancement
rule, `RegisterLatest` was not started.

Provisional smokes were pinned with `taskset -c 0` on an AMD Ryzen 9 5900XT;
the recorded governor/driver were `schedutil` / `acpi-cpufreq`. Canonical
five-run measurement and hardware counters remain pending.

Rejected attempts:

- Per-instruction ExecutionState direct emission measured 5.263503275s. Block
  batching reduced this to 3.612815034s; moving the syscall acceleration cache
  into the common staged runtime reduced it to 3.004557075s.
- Reusing `PureCountedBlock` instruction bodies inside the preflight entry ABI
  caused a focused-test SIGSEGV because the two styles have different resident
  register contracts. The experiment was reverted before any timing artifact
  was accepted.
- A subsequent internal ExecutionState body retained the preflight ABI while
  eliminating per-instruction next-PC publication; it passed parity and was
  retained, reducing ExecutionState from 3.004557075s to 2.785141804s.

### Planner attribution checkpoint

The durable source revisions are Ceno `f73dc3f4` and benchmark/ledger
`78dd9c43`. A cached canonical-cell Planner profile measured 5.353398779s with
the exact hash, instruction count, final cycle and 35 shards. The untruncated
999 Hz profile assigns 3,616 sampled periods to the Planner DSO versus 1,227
for ExecutionState. The 2,389 added DSO periods are classified as:

| Generated region | Planner periods | ExecutionState periods | Added periods |
|---|---:|---:|---:|
| Accounting | 1,059 | 0 | 1,059 |
| Guest memory body | 1,054 | 673 | 381 |
| Guest arithmetic/control body | 947 | 430 | 517 |
| Guards | 393 | 72 | 321 |
| Plan commit | 58 | 0 | 58 |
| Dispatch | 103 | 52 | 51 |

This is observed sampling evidence, not an elapsed-time decomposition. Full
reports and logs are under
`.codex-results/aot-tracking-20260810/planner-profile-abi59/` and
`.codex-results/aot-tracking-20260810/execution-profile-abi59/`.

ABI 60 temporarily reused the resident-next-PC ExecutionState body for
Planner block-plan bodies. Focused tests passed, but the canonical block run
trapped in the busy-loop guard before the target metric. The experiment is an
invalid trim and was fully reverted; no timing result was accepted. The source
returned byte-clean to `f73dc3f4`.

Assembly inspection of a hot accounting region shows deterministic block
contributions still loaded from descriptor/contribution tables and scanned in
a runtime loop on every planned block entry. The next source-supported design
is to place the cost-model identity in the cache key, generate those immutable
block descriptors at AOT construction time, and specialize/unroll the common
one- or two-chip admission path. This follows the compile-time descriptor
architecture without repeating the failed register-only fusion, duplicate
scan, or resident-base experiments.

### Planner descriptor specialization (ABI 61)

Ceno commit `7c79627d` implements the compile-time descriptor design. The
artifact identity includes deterministic shard-cost-model fingerprint
`46a252f9299cfc23d4d539af42c5dc115ebf2b9756f3660bd7672b8dd35ad203`,
which covers opcode and ecall chip mappings, chip specifications, generated
trace/main/tower tables, and extension degree. Runtime rejects a cached
planner artifact if its model fingerprint differs.

One- and two-contribution blocks embed chip indices, instance deltas, cost-row
offsets, and cold split standalone costs as immediates. Their bucket proof,
rollback, and exact transition paths are unrolled. Blocks with more than two
contributions retain the ABI-59 generic descriptor loop. Production Preflight
and staged `Full` use the same internal selection and remain byte-identical.
The ABI is 61 rather than reusing ABI 60, which remains the rejected
resident-next-PC experiment.

Canonical block-25580200 evidence at 4.5B cells:

- Cold ABI-61 execution: `5.026433376s` total, `3.585856608s` native,
  `1.440576768s` fallback. Against the ABI-59 canonical `5.381945825s`
  sample this is `0.355512449s` or `6.61%` faster, clearing both retention
  thresholds.
- Cached ABI-61 execution: `4.825796107s` total, `3.397309932s` native,
  `1.428486175s` fallback. This is a provisional single cached sample, not a
  five-run median.
- The matched `cycles:u` profiling execution was `5.215572346s` total,
  `3.685349039s` native, and `1.530223307s` fallback. It is a counter artifact,
  not a timing sample.
- The cold and cached samples both produced the required hash, exit code 0,
  exactly `994,896,527` instructions, final cycle `3,979,586,112`, 35 shards,
  35 predicted costs, and the complete unchanged 36-point boundary vector.
- Setup was `128.041592293s` and compile/load was `52.382106018s` on the cold
  run; cached setup was `404.774189ms` and load was `70.081µs`. These remain
  outside `run_to_halt` timing.
- ABI-61 artifact size is `115,387,832` bytes versus `114,359,736` bytes for
  ABI 59, a `0.90%` increase.

Matched untruncated 999 Hz `cycles:u` profiling records 938 accounting samples
in the ABI-61 Planner DSO versus 1,059 for ABI 59, a reduction of 121 samples
or `11.4%`. Total generated-DSO samples also fall by exactly 121, from 3,616
to 3,495. The other classified regions move from/to: memory 1,054/1,071,
guest 947/975, guards 393/372, plan commit 58/46, and other 105/93. Thus the
accounting reduction is not displaced into guards, guest bodies, memory,
commit, or callbacks. This is sampling attribution, not elapsed-time
decomposition. A specialized hot-block disassembly contains no descriptor
scan loop; a sampled block with more than two contributions retains it as
intended. Raw logs, the untruncated report, perf data, and disassemblies are in
`.codex-results/aot-tracking-20260810/planner-specialized-abi61/`.

Validation completed:

- `cargo fmt --check`
- complete `ceno_emul` suite with `aot-x86_64`: 76 passed, 1 ignored
- finite-cycle and finite-cell shard tests
- specialized-versus-generic finite-cell planner state, costs, boundaries,
  callbacks, and tape parity
- direct-syscall parity and cache hit/corruption/identity rebuild tests
- all cumulative tracking-stage checks
- production-`Full` byte-identical emitter equality with specialized metadata
- release benchmark build and two canonical cached-block executions

The broader `ceno_zkvm` AOT test build was attempted but stopped while
compiling its separate debug graph because the filesystem had no free space;
it did not reach test execution. No unrelated artifacts were deleted to retry.
This is an infrastructure failure, not passing evidence.

The cold layout profile measured 129,344,238 static edge executions, of which
84,702,885 (`65.49%`) follow adjacent emitted edges. This supports wider hot
conditional/native-memory region work, but descriptor specialization alone
does not pass the budget gate: the cached Planner prefix consumes
`2.383930001s`, leaving `0.242069999s` before `1.500s` of remaining minimum
allocations. Advancement therefore stops at Planner. A region implementation
must use the exact per-block path whenever every affected chip cannot remain
inside its cached bucket or the complete region cannot fit the current shard,
and must commit the exact executed prefix on early exit.

### RegisterLatest initialized-register guard (ABI 62)

The user explicitly directed the campaign to continue through the remaining
ladder despite the failed Planner projection. This supersedes the procedural
advancement stop, but does not relax the exactness or final `<5.0s` acceptance
gates.

The first unmodified ABI-61 `RegisterLatest` artifact measured
`6.206094958s` cold and `6.166198925s` cached. Ceno commit `cb3cc5b9` makes the
existing initialized-register mask effective in the no-event stage: if every
register first-used by a block is already globally initialized, generated code
skips all per-register zero probes and first-touch-length publication. The
exact cold probe remains for unseen registers, and block-last latest-cycle
commits are unchanged. The AOT ABI advances to 62.

Canonical block-25580200 evidence at 4.5B cells:

- Cold ABI-62 execution: `5.529363466s` total, `3.972288235s` native,
  `1.557075231s` fallback.
- Cached ABI-62 execution: `5.465545408s` total, `3.930704220s` native,
  `1.534841188s` fallback.
- Against the cached unmodified `6.166198925s` sample, the cached candidate is
  `0.700653517s` (`11.36%`) faster, clearing both retention thresholds.
- Both runs preserve the required hash, exit code 0, exactly `994,896,527`
  instructions, final cycle `3,979,586,112`, all 35 shards, and the complete
  unchanged 36-point boundary vector.
- The cached adjacent marginal against Planner's provisional
  `4.825796107s` is `+0.639749301s`; cumulative overhead against Pure is
  `+3.023679302s`, leaving `-0.397679302s` of the nominal tracking budget.
  These are provisional single-sample ledger values, not alternating medians.

The separate `cycles:u` run measured `5.479519201s` and remained exact. Its
untruncated 999 Hz generated-code classification assigns 28 samples to
register-first checks and 171 to register-latest commits; accounting has 942,
guards 403, and plan commit 52. Thus the retained change removes the intended
first-touch work. The remaining stage delta is not wholly attributable to the
register-labelled regions and includes generated-layout/body displacement;
this is sampling evidence rather than an elapsed-time decomposition.

Validation completed:

- `cargo fmt --check`
- complete `ceno_emul` suite with `aot-x86_64`: 76 passed, 1 ignored, with the
  two integration tests also passing
- cumulative tracking-stage parity and later-responsibility exclusion
- finite-cell/cycle shards, direct-syscall parity, cache corruption/rebuild,
  specialized planner parity, and production-`Full` emitter equality
- release benchmark rebuild and exact cold/cached canonical executions

Raw logs and the untruncated profile are under
`.codex-results/aot-tracking-20260810/register-latest-mask-abi62/`.

### MemoryLatest checkpoint (ABI 62)

The existing cumulative emitter was measured without a source change. Cold
execution was `5.780129189s` total (`4.175448969s` native and
`1.604680220s` fallback); the cached execution was `5.747544993s` total
(`4.147496362s` native and `1.600048631s` fallback). Both preserve the
required hash, exit code, instruction count, final cycle, and complete
35-shard/36-boundary planner vector.

The cached adjacent marginal is `+0.281999585s` over RegisterLatest, below the
stage's `0.600s` allocation. Cumulative overhead against Pure is
`+3.305678887s`, leaving `-0.679678887s` of the nominal budget. A hot memory
block disassembly shows the expected packed-cell read, high-half ordinal
decode, value preservation, and new high-half ordinal store. Mmio extrema,
event-capacity, and next-access tape append work remain absent. Because the
observed marginal is already below half its allocation and the earlier causal
packed-stamp floor was approximately `0.444s`, no speculative stage-local
rewrite was justified. Artifacts are under
`.codex-results/aot-tracking-20260810/memory-latest-abi62/`.

### MmioBounds hoisted-region reuse (ABI 63)

Ceno commit `f12cb498` removes duplicate memory validation from block-planned
native bodies. The block-entry guard remains authoritative for admission and
records each of at most 32 memory accesses as a two-bit heap, stack, or hints
region. Mmio code consumes that immutable map to update only the proven
region's extrema; blocks outside the safe bound retain the generic path. The
AOT ABI advances to 63.

The unmodified ABI-62 cold baseline was `7.696334669s`. The ABI-63 cold
candidate was `7.091111770s` (`5.581386519s` native and `1.509725251s`
fallback), an improvement of `0.605222899s` (`7.86%`), clearing both retention
thresholds. A subsequent cache-hit sample was noisy at `7.816615748s`; it is
recorded as raw evidence and is not substituted for the controlled cold pair.

Because ABI 63 also eliminates redundant alignment and dense-membership tests
before MmioBounds, MemoryLatest was regenerated at the same ABI. Its cold
execution was `5.663222366s`, making the cold adjacent Mmio marginal exactly
`1.427889404s`. Relative to Pure, cumulative overhead is `4.649245664s`, with
`-2.023245664s` nominal budget remaining. The reconciliation residual is zero:
`7.091111770 - 5.663222366 = 1.427889404` and
`7.091111770 - 2.441866106 = 4.649245664`.

All canonical runs preserve the exact block hash, exit status 0,
`994,896,527` instructions, final cycle `3,979,586,112`, and complete
35-shard/36-boundary vector. The complete `ceno_emul` AOT suite passed (76
tests, 1 ignored, plus 2 integration tests). Hot disassembly shows the encoded
region load and selection with no repeated address alignment or region-range
classification in the memory body; later capacity/event work remains absent.

An ABI-64 experiment validated the six extrema pointers once and outlined rare
extrema updates. It stayed exact but regressed cold execution to
`7.278571988s`, so it was reverted and is not part of the retained commit. Raw
logs and disassembly are under
`.codex-results/aot-tracking-20260810/{memory-latest-abi63,mmio-regions-abi63,mmio-cold-updates-abi64}/`.

### EventCapacity checkpoint (ABI 63)

The existing emitter measured `7.840320821s` cold (`6.206596696s` native and
`1.633724125s` fallback), an adjacent marginal of `0.749209051s` over the
ABI-63 MmioBounds cold sample. Cumulative overhead against Pure is
`5.398454715s`, leaving `-2.772454715s` of nominal budget. The run produced no
events, retained the trained capacity of `18,973,696`, performed no growth,
and preserved all canonical correctness fields and shard boundaries. The
arithmetic reconciles exactly:
`7.840320821 - 7.091111770 = 0.749209051` and
`7.840320821 - 2.441866106 = 5.398454715`.

An ABI-64 diagnostic-stage specialization omitted per-block capacity checks
because the EventCapacity cursor never advances. It remained exact and
measured `7.631286526s`, only `0.209034295s` (`2.67%`) faster. This missed both
retention thresholds, so the code and ABI bump were reverted. RegisterEvents
and Full were never changed and retain their exact production growth guards.
Raw evidence is under
`.codex-results/aot-tracking-20260810/event-capacity-abi{63,64}/`.

### RegisterEvents checkpoint (ABI 63)

The existing register-event emitter measured `7.922237605s` cold
(`6.276199104s` native and `1.646038501s` fallback). The exact adjacent
marginal is `0.081916784s`, below the stage's `0.150s` allocation, so no
speculative rewrite was justified. Cumulative overhead against Pure is
`5.480371499s`, leaving `-2.854371499s` of nominal budget; the reconciliation
residual is zero.

The run finalized exactly 1,086 register events (32 initialization and 1,054
non-initial), with capacity `18,973,696`, no growth, and no normal-access
callbacks. Hash, exit status, instruction count, final cycle, planner state,
and the full shard vector remain exact. Hot disassembly contains the expected
three-field 24-byte register tape stores and no memory-event labels. Evidence
is under `.codex-results/aot-tracking-20260810/register-events-abi63/`.

### Full checkpoint (ABI 63)

The production-equal Full artifact measured `7.845179490s` cold
(`6.315530745s` native and `1.529648745s` fallback). It finalized exactly
`16,865,461` events (`14,172,036` initialization and `2,693,425`
non-initial), matching training, with no growth or normal-access callbacks.
Finalization remained outside `run_to_halt` and took `580.728909ms`.

The adjacent delta from RegisterEvents is `-0.077058115s`, retained as
measurement/layout noise rather than negative consumed work. Full cumulative
overhead against Pure is exactly `5.403313384s`; nominal budget remaining is
`-2.777313384s`, with zero arithmetic residual. The exact hash, exit status,
instruction count, final cycle, planner state, and complete 35-shard vector
all pass, and the focused assembly test proves staged Full is byte-identical
to the production emitter. The endpoint performance acceptance fails:
`7.845179490s` is `2.845179490s` above the required `<5.0s` threshold.
Evidence is under `.codex-results/aot-tracking-20260810/full-abi63/`.
