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

## Stage 2 — Generify the problem-agnostic core ⬜

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

### Deliverables
- Generic `Separator`, `SeparatorWorker`, `SolutionListener`.
- SPP behavior bit-identical (or near-identical) to baseline.

---

## Stage 3 — BPP problem impl + LBF builder ⬜

### Tasks
1. **3.1** Implement `PackingProblem + BinCapacity` for `BPProblem`.
2. **3.2** Make [src/optimizer/lbf.rs](src/optimizer/lbf.rs) generic: `LBFBuilder<P>`.
3. **3.3** Introduce a small strategy trait for "what to do when an item won't fit":
   ```rust
   trait CapacityFailureStrategy<P> {
       fn on_place_failure(&self, prob: &mut P, item_id: usize) -> Result<(), &'static str>;
   }
   ```
   - SPP impl: `prob.change_strip_width(prob.strip_width() * 1.2)`.
   - BPP impl: pick a bin type (best historical density, or smallest that fits item bounding circle) and `prob.open_bin(...)`. Returns `Err` if no bin type can possibly hold the item.
4. **3.4** Smoke test: build initial BPP solution from a tiny synthetic instance.

### Deliverables
- Working BPP construction; produces a feasible (possibly bin-count-suboptimal) initial solution.

---

## Stage 4 — BPP exploration & compression algorithms ⬜

**Why:** Strip-shrink has no analog. Need new algorithms that drive bin count down using Sparrow's overlap-proxy machinery.

### 4.1 Exploration: `src/optimizer/explore_bpp.rs`
Loop until time/iter budget exhausted:
1. Pick the **least-loaded bin** $b^*$ (by area density).
2. Snapshot solution.
3. Free all items in $b^*$ → `close_bin(b*)`.
4. Re-insert each freed item by injecting it into the **best-fit** remaining bin (allowing initial overlap).
5. Run `Separator::separate()` to resolve overlaps under the existing overlap-proxy + adaptive-weight scheme.
6. If feasible within budget → accept (one fewer bin); else rollback and either:
   - try a different bin to remove,
   - escalate (increase iter budget),
   - or terminate exploration.

### 4.2 Compression: `src/optimizer/compress_bpp.rs`
Operate on already-feasible layout. Cheap moves:
- **Bin-emptying**: greedy attempt to evacuate the emptiest bin into others (short separator burst).
- **Bin-merging**: pick two low-density bins, virtually treat as one doubled-capacity bin, separate, accept if all fit in one physical bin.

### 4.3 Config
Add to [src/config.rs](src/config.rs):
- `BppExplorationConfig { bin_removal_attempts, attempt_iter_budget, ... }`
- `BppCompressionConfig { merge_attempts, evacuation_iter_budget, ... }`

### Deliverables
- Two new files plus config additions; SPP modules untouched.

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

