# Sparrow → BPP Extension Plan

> **Goal:** Extend `sparrow` (currently a 2D irregular **Strip Packing (SPP)** heuristic) to also solve **Bin Packing (BPP)** — both single-bin and multi-bin variants — by reusing the existing collision/quantification/sampling/separator machinery and replacing only the strip-specific orchestration with bin-oriented orchestration.
>
> **Non-goal (for now):** LP/MILP-based exact methods. Those are explored in a separate document; this plan focuses on a clean BPP heuristic that mirrors Sparrow's two-phase explore→compress structure.

---

## Status legend

- ⬜ Not started
- 🔄 In progress
- ✅ Complete
- ⚠️ Blocked / needs decision

---

## Architectural baseline (current state)

### Already problem-agnostic — reused as-is
| Module | Operates on |
|---|---|
| [src/quantify/](src/quantify/) (incl. SIMD) | Pure shape/position math |
| [src/sample/](src/sample/) | `Layout` + `Item`, returns `DTransformation` |
| [src/eval/](src/eval/) | `Layout` + `Item` |
| Inner separator loop (`Separator::separate`, `move_items`) | `Layout` + `CollisionTracker` |

### Strip-bound — must be reworked or abstracted
| Location | Strip-specific responsibility |
|---|---|
| [src/optimizer/explore.rs](src/optimizer/explore.rs) | Iteratively shrinks strip width |
| [src/optimizer/compress.rs](src/optimizer/compress.rs) | Random-split shrink at endgame |
| [src/optimizer/separator.rs](src/optimizer/separator.rs) `change_strip_width` | Horizontally shifts items across a split |
| [src/optimizer/lbf.rs](src/optimizer/lbf.rs) | On placement failure: grows strip 1.2× |
| [src/util/io.rs](src/util/io.rs), [src/main.rs](src/main.rs), [src/bench.rs](src/bench.rs) | SPP-only JSON & CLI |
| [src/util/svg_exporter.rs](src/util/svg_exporter.rs) | Single-container SVG |
| [src/util/listener.rs](src/util/listener.rs) | `SolutionListener` typed on `SPSolution` |
| [src/util/assertions.rs](src/util/assertions.rs) | `strip_width_is_in_check` |

---

## Stage 0 — Verify jagua-rs BPP API surface ✅

**Why:** Everything downstream depends on the exact shape of `jagua_rs::probs::bpp`. We must confirm the types, methods, IO functions, and JSON format **before** designing traits.

### Tasks
1. **0.1** Add `"bpp"` to the `jagua-rs` features in [Cargo.toml](Cargo.toml) (keeping `"spp"`).
2. **0.2** Run `cargo check` to confirm both features compile together.
3. **0.3** Locate the jagua-rs source in the cargo registry cache (`~/.cargo/registry/src/.../jagua-rs-0.7.1/`) and inspect:
   - `jagua_rs::probs::bpp::entities` — exact names of `BPInstance` / `BPProblem` / `BPSolution` / `BPPlacement` and their public fields/methods.
   - `BPProblem` API for: bin add/remove/open/close, listing layouts, item placement (target layout selection), `save`/`restore`.
   - `jagua_rs::probs::bpp::io` — `ExtBPInstance`, `ExtBPSolution`, `import_instance`, `import_solution`, `export`.
   - JSON schema differences vs. SPP (presence/shape of `"bins"` field, etc.).
4. **0.4** Write a throwaway scratch binary `examples/bpp_smoke.rs` that:
   - Constructs a minimal `BPInstance` (via JSON or programmatically).
   - Creates a `BPProblem`, places 1–2 items, prints layouts.
   - Calls `save()` then `restore()` and verifies round-trip.
   - Round-trips through `bpp::io::import_instance` / `export` if a tiny BPP JSON is available.
5. **0.5** Document the discovered API in a "Stage 0 findings" section appended to this file.
6. **0.6** **Decision gate:** if BPP is missing, broken, or lacks key methods (e.g., no programmatic bin-removal), pause and decide between (a) upstream PR, (b) pinning to a git branch with the needed API, or (c) reduced-scope plan.

### Deliverables
- Updated `Cargo.toml`.
- `examples/bpp_smoke.rs` (kept in repo until Stage 5 cleanup).
- Stage 0 findings appended below.

---

## Stage 1 — Trait abstraction (no behavior change) ✅

**Why:** Decouple the optimizer from `SPProblem` without touching algorithms.

### Tasks
1. **1.1** Create `src/optimizer/problem.rs` with:
   ```rust
   pub trait PackingProblem {
       type Instance;
       type Solution: Clone;
       type Placement;
       fn instance(&self) -> &Self::Instance;
       fn layouts(&self) -> &[Layout];
       fn layouts_mut(&mut self) -> &mut [Layout];
       fn place_item(&mut self, p: Self::Placement);
       fn remove_item(&mut self, layout_idx: usize, key: PItemKey);
       fn save(&mut self) -> Self::Solution;
       fn restore(&mut self, sol: &Self::Solution);
       fn density(&self) -> f32;
   }
   pub trait StripCapacity: PackingProblem {
       fn strip_width(&self) -> f32;
       fn change_strip_width(&mut self, w: f32, split: Option<f32>);
       fn fit_strip(&mut self);
   }
   pub trait BinCapacity: PackingProblem {
       fn n_bins(&self) -> usize;
       fn open_bin(&mut self, bin_type_id: usize) -> usize; // returns layout idx
       fn close_bin(&mut self, layout_idx: usize);          // requires emptied
   }
   ```
2. **1.2** Implement `PackingProblem + StripCapacity` for `SPProblem` as thin wrappers.
3. **1.3** Verify `cargo build` and `cargo test` are green; **no consumer changes** in this stage.

### Deliverables
- New `src/optimizer/problem.rs`.
- Existing tests still pass unchanged.

### Stage 1 result (2026-04-24)
- Added [src/optimizer/problem.rs](src/optimizer/problem.rs): `PackingProblem`, `StripCapacity`, `BinCapacity` traits + `SPProblem` impl.
  - `LayoutKey` is an associated type (`()` for SPP, will be `LayKey` for BPP) — adjusted from the original plan to match jagua-rs's slotmap-keyed layouts.
  - Note: `change_strip_width(_, split_pos)` (the item-shifting variant) is **not** on the trait — it lives on `Separator` because it manipulates collision-tracker state, not just the strip. The trait's `change_strip_width` mirrors `SPProblem::change_strip_width` (single arg).
- [src/optimizer/mod.rs](src/optimizer/mod.rs): added `pub mod problem;` (only line changed).
- New unit test `optimizer::problem::tests::spp_satisfies_packing_problem_trait` ✅
- `cargo build --all-targets` ✅
- `cargo test --release --tests` → 3 passed, 0 failed (full integration suite, no regression).

---

## Stage 2 — Generify the problem-agnostic core ✅ (revised scope)

**Why:** Make `Separator`, `SeparatorWorker`, and `SolutionListener` generic over `P: PackingProblem` while keeping SPP behavior identical.

### Tasks
1. **2.1** [src/optimizer/separator.rs](src/optimizer/separator.rs): introduce `Separator<P: PackingProblem>` with fields `prob: P`, `cts: Vec<CollisionTracker>` (one per layout — length 1 for SPP).
2. **2.2** Move `change_strip_width` into an extension `impl<P: StripCapacity> Separator<P>`.
3. **2.3** Update every `self.ct.X(...)` site to `self.cts[layout_idx].X(...)`. For SPP code paths, `layout_idx == 0` is hardcoded.
4. **2.4** [src/optimizer/worker.rs](src/optimizer/worker.rs): make `SeparatorWorker<P>` generic. Each worker still bound to a single layout (cross-bin moves deferred to later optimization).
5. **2.5** [src/util/listener.rs](src/util/listener.rs): `SolutionListener<P: PackingProblem>` parameterized by problem type.
6. **2.6** [src/optimizer/mod.rs](src/optimizer/mod.rs): `optimize<P>` becomes generic; SPP path branches on `P: StripCapacity` for the explore/compress phases (Stage 4 wires BPP equivalents).
7. **2.7** Run full SPP test suite + benchmark on `swim.json` to verify zero regression.

### Risks
- `CollisionTracker` per-layout split affects all hot paths; profile after.
- Some `move_items_multi` logic may currently assume a single CT — audit carefully.

### Deliverables (original plan)
- Generic `Separator`, `SeparatorWorker`, `SolutionListener`.
- SPP behavior bit-identical (or near-identical) to baseline.

### Stage 2 result (2026-04-24) — executed with **revised, leaner scope**

**Deviation from the original plan.** After reading the separator/worker code carefully, full generification of `Separator<P>` was judged premature complexity for these reasons:
- `Separator`'s SPP-specific operations (`change_strip_width` with item-shifting at a split position, single `CollisionTracker`, `rollback` checking `strip_width` equality) are deeply baked into the algorithmic flow.
- BPP's separator will be fundamentally different in Stage 4 (per-layout CTs, bin-removal not strip-shrink, no horizontal-shift split). Forcing one generic type across both yields awkward enum-like dispatch with no genuine reuse.
- The truly shared parts ([src/eval/](src/eval/), [src/quantify/](src/quantify/), [src/sample/](src/sample/)) are **already** problem-agnostic — they operate on `Layout` + `Item`, not on `Separator` or `Problem`.

**What was actually delivered:**
- **2A** [src/util/listener.rs](src/util/listener.rs): `SolutionListener<P: PackingProblem>` is now generic; `DummySolListener` is also generic. Updated all 5 consumer sites ([optimizer/mod.rs](src/optimizer/mod.rs), [optimizer/spp/separator.rs](src/optimizer/spp/separator.rs), [optimizer/spp/explore.rs](src/optimizer/spp/explore.rs), [optimizer/spp/compress.rs](src/optimizer/spp/compress.rs), [util/svg_exporter.rs](src/util/svg_exporter.rs)).
- **2B** Carved out an `optimizer::spp` submodule. Moved `lbf.rs`, `separator.rs`, `worker.rs`, `explore.rs`, `compress.rs` under [src/optimizer/spp/](src/optimizer/spp/) with a new [src/optimizer/spp/mod.rs](src/optimizer/spp/mod.rs). Updated all path references in [src/config.rs](src/config.rs), [src/main.rs](src/main.rs), [src/bench.rs](src/bench.rs), [tests/tests.rs](tests/tests.rs), and the cross-references inside the moved files.
- **2C** Renamed `optimize` → `optimize_spp` in [src/optimizer/mod.rs](src/optimizer/mod.rs) and [src/main.rs](src/main.rs). Stage 5 will add `optimize_bpp` and a thin dispatcher.

**Verification:**
- `cargo build --all-targets` ✅
- `cargo test --release --tests -- --test-threads=1` ✅ (3/3 integration tests + 1/1 lib test pass; same as baseline)

**Implication for later stages:** Stage 3 will create a parallel [src/optimizer/bpp/](src/optimizer/bpp/) tree (with its own `lbf.rs`, `separator.rs`, etc.). The duplication of separator code is intentional — the algorithms differ enough that sharing via a single generic `Separator<P>` would obscure both implementations. Where genuinely shared utilities emerge (e.g. CT loss aggregation), they can be extracted later.

---

## Stage 3 — BPP problem impl + LBF builder ✅

### Tasks (revised — see "Stage 3 result" for the deviation rationale)
1. **3.1** Implement `PackingProblem + BinCapacity` for `BPProblem`. ✅
2. **3.2** Create [src/optimizer/bpp/lbf.rs](src/optimizer/bpp/lbf.rs) with a dedicated `BPLBFBuilder` (parallel-tree approach from Stage 2's revised scope). ✅
3. **3.3** Smoke test: drive `BPLBFBuilder::construct()` on `swim.json` from `examples/bpp_smoke.rs`. ✅
4. **3.4** Build + full integration suite green; mark plan. ✅

### Deliverables
- Working BPP construction; produces a feasible (possibly bin-count-suboptimal) initial solution.

### Stage 3 result

**Outcome:** All four substages landed cleanly.

- **3.1 — Trait impls.** [src/optimizer/problem.rs](src/optimizer/problem.rs) now contains `impl PackingProblem for BPProblem` (with `LayoutKey = LayKey`, `Solution = BPSolution`, `Placement = BPPlacement`) and `impl BinCapacity for BPProblem`. Implementations are thin pass-throughs over jagua-rs's existing `BPProblem` API. The associated `Self::LayoutKey` design from Stage 1 was confirmed sufficient — no trait reshaping needed.
- **3.2 — `BPLBFBuilder`.** [src/optimizer/bpp/lbf.rs](src/optimizer/bpp/lbf.rs) implements First-Fit Decreasing:
  - Items sorted by `convex_hull_area * diameter` desc (same key as SPP).
  - For each item: try open layouts in ascending free-area order (tightest fit first), using `LBFEvaluator` + `search_placement` per layout.
  - On no-fit: open the cheapest bin type with stock remaining. The placement search runs against a **scratch `Layout::new(container.clone())`** (an empty stand-in for the to-be-opened bin), then the resulting transform is committed via `BPProblem::place_item(BPPlacement { layout_id: BPLayoutType::Closed { bin_id }, .. })`. This sidesteps the chicken-and-egg of `BPProblem` having no "open empty layout" API — `Closed { bin_id }` always inserts an item, so we needed a feasible transform *before* opening the bin.
  - Failure modes: `BPLBFError::ItemDoesNotFitAnyBin` (no available bin can hold the item) and `OutOfBinStock` (all bin types exhausted).
- **3.3 — Smoke test.** Extended [examples/bpp_smoke.rs](examples/bpp_smoke.rs) to run the builder on `data/input/swim.json` (10 item types, 48 total demand) using a single bin type cloned from the swim strip container. Result: `placed 48/48 items into 2 bin(s), density = 0.500`. The low density is expected — the strip-shaped "bin" is much wider than the items need, so per-bin fill is poor; this is purely a construction smoke test, not a packing-quality benchmark.
- **3.4 — Verification.** `cargo build --all-targets` clean (zero warnings). `cargo test --release --tests -- --test-threads=1` → 1 lib test + 3 integration tests + 0 in `bench`/`sparrow` bin tests, all green. **Zero SPP regression.**

**Key API observation discovered during 3.2:** `BPProblem::place_item(BPPlacement { layout_id: BPLayoutType::Closed { bin_id }, .. })` is atomic — it both opens the new layout and inserts the item in one call. There is no public way to construct an empty layout inside `BPProblem` without an item. The scratch-`Layout` trick (build a free-standing `Layout::new(container.clone())`, run placement search against it, then commit) is the cleanest workaround and worth remembering for Stage 4 (where exploration may want similar "what-if-we-opened-this-bin?" probes).

---

## Stage 4 — BPP exploration & compression algorithms ✅

**Why:** Strip-shrink has no analog. Need new algorithms that drive bin count down using Sparrow's overlap-proxy machinery.

### 4.1 Exploration: `src/optimizer/bpp/explore.rs` ✅
Loop until time/iter budget exhausted:
1. Pick the **least-loaded bin** $b^*$ (by area density).
2. Snapshot solution.
3. Free all items in $b^*$ → `remove_layout(b*)`.
4. Re-insert each freed item by injecting it into the **best-fit** remaining bin (allowing initial overlap).
5. Run `BPSeparator::separate()` to resolve overlaps under the existing overlap-proxy + adaptive-weight scheme.
6. If feasible within budget → accept (one fewer bin); else rollback and blacklist this bin for the rest of the run.

### 4.2 Compression: `src/optimizer/bpp/compress.rs` ⬜ (deferred to Stage 5+)
Strip-shrink has no direct BPP analog. Plausible BPP analogs (bin-emptying, bin-merging) are deferred to a later iteration so Stage 4 lands a working end-to-end pipeline first. The orchestrator [`optimize_bpp`](src/optimizer/mod.rs) currently runs LBF → exploration only.

### 4.3 Config
- `BPExplorationConfig { max_bin_removal_attempts, time_limit }` lives in [src/optimizer/bpp/explore.rs](src/optimizer/bpp/explore.rs) for v1. The exploration reuses `SeparatorConfig` from the SPP module (it carries iteration limits + sample config that apply unchanged).

### Stage 4 result

**Outcome:** End-to-end BPP pipeline lands. Smoke test runs LBF + exploration on swim.json without panic.

**Files added/changed:**
- [src/optimizer/bpp/separator.rs](src/optimizer/bpp/separator.rs) — `BPSeparator` with a per-layout `SecondaryMap<LayKey, CollisionTracker>` and `BPSepSnapshot` (problem snapshot + per-layout CT snapshots). Sequential single-layout sweeps; reuses `SeparationEvaluator` and `search::search_placement` unchanged.
- [src/optimizer/bpp/explore.rs](src/optimizer/bpp/explore.rs) — `bpp_exploration` + `BPExplorationConfig`. Best-fit injection (largest item first into the layout with most free area), LBF search per item with a centroid-fallback for overlap-tolerant placement, blacklist of failed-removal layouts.
- [src/optimizer/mod.rs](src/optimizer/mod.rs) — `optimize_bpp(instance, rng, sol_listener, terminator, expl_config, sep_config) -> BPSolution`. Pipeline: `BPLBFBuilder::construct` → `BPSeparator::new` → `bpp_exploration`.
- [examples/bpp_smoke.rs](examples/bpp_smoke.rs) — extended with a fourth section that drives `optimize_bpp` end-to-end.

**V1 limitations (intentional, recorded in source comments):**
1. **Serial separator.** The SPP separator uses a rayon `SeparatorWorker` pool that races N copies and keeps the best. The BPP separator runs one sweep per layout in sequence. Parallelism (per-layout or per-worker) is a Stage 6+ optimization.
2. **No cross-bin moves inside the separator.** Items only relocate across bins via the exploration phase's redistribution step. This matches the cross-cutting risk #2 in the plan.
3. **Single-item layouts are not separated.** A move = `remove_item` + `place_item`; if the layout had only one item, `remove_item` auto-closes the layout and invalidates its `LayKey`, breaking CT bookkeeping. The separator skips such layouts (a single-item layout has no pair collisions and any container collision would need cross-bin movement to resolve, which is the exploration's job).
4. **No compression phase.** Skipped for v1; the orchestrator goes straight from exploration to `Final` report.
5. **Simple blacklist.** Once a bin fails removal, it's skipped for the remainder of the run. There's no infeasible-solution pool / disrupt mechanism (the SPP analog has no obvious BPP shape: in BPP the "current state" is a set of bins, not a width).

**Smoke test outcome (`cargo run --release --example bpp_smoke`):**
- Section 1-2: BPInstance build + place/save/restore round-trip — OK.
- Section 3: `BPLBFBuilder` placed 48/48 swim items into 2 bins at density 0.500 — OK.
- Section 4: `optimize_bpp` ran LBF then 4 bin-removal attempts. Final bin count remained 2 — **expected**: the swim items at strip-shaped containers fill each bin to ~50%, so dropping to 1 bin would require 100% density, which is geometrically infeasible. The exploration correctly attempted, failed, rolled back, and blacklisted. No panics, no item loss, separator state stayed coherent across snapshot+restore cycles.

**Verification:**
- `cargo build --all-targets` — clean, zero warnings.
- `cargo test --release --tests -- --test-threads=1` — 1 lib test + 3 integration tests (shirts, swim, trousers), all green. **Zero SPP regression.**

**Key API observations from this stage** (worth carrying into Stage 5+):
- `BPProblem::remove_layout(lkey)` works as documented; it cleanly drops the layout and frees demand counters.
- `BPSolution::layout_snapshots.len()` is the canonical "how many bins are open" measure post-restore — much cheaper than walking the problem.
- `BPSepSnapshot` (problem snapshot + per-layout CT snapshots) needs to handle both **stale** keys (layout removed since snapshot) and **missing** keys (layout opened since snapshot) on restore. The current implementation rebuilds CTs from scratch for any layout that doesn't have a snapshot entry — correct but conservative; could be optimized later.

---

## Stage 5 — Wiring: config, IO, CLI, SVG ⬜

### Tasks
1. **5.1** [src/config.rs](src/config.rs): `enum ProblemKind { Spp, Bpp }` and parallel config blocks.
2. **5.2** [src/util/io.rs](src/util/io.rs): add `read_bpp_input`. Detect kind from JSON shape (presence of `"bins"`/`"objects"` array vs. `"strip"` field). Return `enum LoadedInstance { Spp(...), Bpp(...) }`.
3. **5.3** [src/main.rs](src/main.rs): match on loaded kind; dispatch to `optimize::<SPProblem>` or `optimize::<BPProblem>`.
4. **5.4** [src/util/svg_exporter.rs](src/util/svg_exporter.rs): for BPP, emit `final_{name}_bin_{i}.svg` per layout plus an index file.
5. **5.5** [src/bench.rs](src/bench.rs): add `--problem` flag; primary KPI becomes `final_strip_width` (SPP) or `final_bin_count` + `total_used_area` (BPP).
6. **5.6** Delete `examples/bpp_smoke.rs` (or keep as a doctest fixture).

---

## Stage 6 — Tests & assertions ⬜

### Tasks
1. **6.1** Add a tiny BPP instance under `data/input/` (e.g. `swim_bpp.json` derived by giving `swim.json` a fixed bin size).
2. **6.2** Add an integration test mirroring [tests/tests.rs](tests/tests.rs) for BPP.
3. **6.3** Add `bin_count_is_in_check` to [src/util/assertions.rs](src/util/assertions.rs): assert `n_bins ≤ Σ items_area / min_bin_area + slack`.
4. **6.4** Run full benchmark suite and update [data/experiments/README.md](data/experiments/README.md) with BPP results.

---

## Cross-cutting risks & open questions

1. **Multi-`CollisionTracker` overhead.** The CT carries an O(n²) loss matrix per layout. Splitting per-bin actually reduces matrix size but adds a per-move dispatch cost. Profile after Stage 2.
2. **No cross-bin moves in workers (Stage 2 limitation).** Cross-bin item relocation happens only via the exploration redistributor in Stage 4. Adding cross-bin sampling to workers is a future optimization (Stage 7+).
3. **Heterogeneous bin sizes.** If the instance has multiple bin types, `open_bin` needs a selection policy. Initial heuristic: best historical density per type; refine later.
4. **`change_strip_width(_, Some(split))` semantics** in [src/optimizer/separator.rs:218-235](src/optimizer/separator.rs#L218) is the only place items move outside `place_item`. After Stage 2 it lives behind `StripCapacity` and never leaks into BPP code.
5. **JSON discrimination.** If SPP and BPP JSON share top-level fields, detection in `io.rs` must be robust — explicit `"problem_type"` field is preferable; otherwise sniff distinguishing fields.

---

## Suggested first PR

**Stage 0 + Stage 1 only.** Verify BPP API exists, define traits, implement them for `SPProblem`, no consumer changes. ~150 lines, fully reversible, unblocks all later work.

---

## Stage 0 findings

**Date:** 2026-04-24 · **jagua-rs version:** 0.7.1 · **Result:** all required APIs present and behave as documented. Smoke test passes.

### What was changed in this stage
- [Cargo.toml](Cargo.toml): enabled `bpp` feature on `jagua-rs` (now `features = ["spp", "bpp"]`).
- Added [examples/bpp_smoke.rs](examples/bpp_smoke.rs): builds a `BPInstance` from `swim.json`'s items + the SPP strip as a single bin, then exercises `place_item` / `save` / `remove_item` / `restore`. Output:
  ```
  [smoke] BPInstance: 10 item types, 1 bin types, total demand = 48
  [smoke] placed item 0 -> layout LayKey(1v1), pi PItemKey(1v1)
  [smoke] save() captured 1 layout(s)
  [smoke] restore() round-tripped successfully
  [smoke] OK
  ```

### BPP API surface (jagua-rs 0.7.1)

**Module path:** `jagua_rs::probs::bpp`

#### `entities`
| Type | Notes |
|---|---|
| `BPInstance { items: Vec<(Item, usize)>, bins: Vec<Bin> }` | `bins[i].id` must equal `i` (consecutive from 0). Asserted in `BPInstance::new`. |
| `Bin { id, container, stock, cost }` | `id == container.id`. `stock` = max copies; `cost` per use. |
| `BPProblem { instance, layouts: SlotMap<LayKey, Layout>, item_demand_qtys, bin_stock_qtys }` | Layouts are keyed by `LayKey` (slotmap key), **not** by index. |
| `BPSolution { layout_snapshots: SecondaryMap<LayKey, LayoutSnapshot>, time_stamp }` | Restorable snapshot. |
| `BPPlacement { layout_id: BPLayoutType, item_id, d_transf }` | |
| `enum BPLayoutType { Open(LayKey), Closed { bin_id: usize } }` | `Closed` variant **auto-opens a new bin** of the given type on `place_item`. |
| `LayKey` | `slotmap::new_key_type!`, public. |

#### `BPProblem` methods (verified)
| Method | Behavior |
|---|---|
| `new(instance) -> Self` | Empty problem; no layouts yet. |
| `place_item(BPPlacement) -> (LayKey, PItemKey)` | If `Closed { bin_id }`, opens new layout from that bin type. |
| `remove_item(LayKey, PItemKey) -> BPPlacement` | If layout becomes empty, **auto-closes** it; returned placement carries `Closed { bin_id }`. |
| `remove_layout(LayKey)` | Closes a layout (and frees its items' demand counters). |
| `save() -> BPSolution` | Snapshot. |
| `restore(&BPSolution) -> bool` | Returns `true` if any layout keys changed. Reopens missing layouts. |
| `density() -> f32` | Total item area / total bin area used. |
| `bin_cost() -> u64` | Sum of `cost * used_qty` per bin type. |
| `n_placed_items() -> usize` | |
| `item_placed_qtys()` / `bin_used_qtys()` | Iterators. |

#### `io`
| Item | Status |
|---|---|
| `import_instance(&Importer, &ExtBPInstance) -> Result<BPInstance>` | ✅ available |
| `export(&BPInstance, &BPSolution, EPOCH) -> ExtBPSolution` | ✅ available (signature pattern matches SPP) |
| `import_solution(&BPInstance, &ExtBPInstance) -> BPSolution` | ⚠️ **`unimplemented!()` in 0.7.1** — warm-starting from a BPP solution JSON is not possible. |
| `ExtBPInstance { name, items: Vec<ExtItem>, bins: Vec<ExtBin> }` | JSON shape. |
| `ExtBPSolution { cost, layouts: Vec<ExtLayout>, density, run_time_sec }` | JSON shape. |

### Implications for the plan

1. **No behavioral blockers.** `place_item` / `remove_item` / `save` / `restore` cover everything Sparrow needs; `BPLayoutType::Closed` cleanly handles the "open a new bin" case for the LBF builder (Stage 3).
2. **`PackingProblem` trait shape (Stage 1) needs a small adjustment.** Layouts are accessed via a `SlotMap<LayKey, Layout>`, **not** a `&[Layout]`. The trait signature should be:
   ```rust
   fn layouts(&self) -> impl Iterator<Item = (Self::LayoutKey, &Layout)>;
   fn layouts_mut(&mut self) -> impl Iterator<Item = (Self::LayoutKey, &mut Layout)>;
   fn layout(&self, key: Self::LayoutKey) -> &Layout;
   ```
   For SPP, `LayoutKey = ()` (always one layout); for BPP, `LayoutKey = LayKey`. This affects Stage 2's per-layout `CollisionTracker` storage: prefer `SecondaryMap<LayKey, CollisionTracker>` for BPP.
3. **Bin id normalization.** When constructing a BPP instance from arbitrary sources, container ids must be 0..N consecutive (asserted hard). Document this in Stage 5's IO layer.
4. **No warm-start for BPP.** Stage 5's `read_bpp_input` should accept only an `ExtBPInstance` (no solution JSON). Either upstream a fix to jagua-rs `import_solution` or skip warm-start for BPP entirely. **Decision:** skip for v1; revisit after Stage 6.
5. **`SPInstance::containers()` returns empty** — the strip becomes a `Container` only inside `SPProblem::new`. This is the reason the smoke test had to grab the container via `SPProblem::new(...).layout.container`. Not a problem for this codebase but documents an asymmetry between SPP and BPP.

### Decision gate result
**PROCEED to Stage 1.** No upstream changes required for the core plan; only loss is the BPP warm-start IO function (acceptable for v1).

