//! Multi-layout separator for the Bin Packing Problem.
//!
//! Each open layout in the [`BPProblem`] gets its own [`CollisionTracker`].
//! Separation is performed **per-layout** in sequence — no item is ever moved
//! across bins by the separator itself (cross-bin redistribution is the job
//! of the exploration phase in [`super::explore`]). This mirrors the
//! single-layout semantics of the SPP separator while keeping each layout's
//! O(n²) collision matrix bounded by *that* layout's item count.
//!
//! V1 limitations (intentional):
//! - Serial (no rayon worker pool). Per-layout parallelism can be reintroduced later.
//! - Single-item layouts are skipped (a move would close-then-reopen the
//!   layout, which would invalidate its `LayKey` and make CT bookkeeping
//!   awkward; not worth the complexity for v1).

use crate::eval::sep_evaluator::SeparationEvaluator;
use crate::optimizer::spp::separator::SeparatorConfig;
use crate::quantify::tracker::{CTSnapshot, CollisionTracker};
use crate::sample::search;
use crate::util::assertions::tracker_matches_layout;
use crate::util::listener::{ReportType, SolutionListener};
use crate::util::terminator::Terminator;
use crate::FMT;
use itertools::Itertools;
use jagua_rs::entities::{Instance, PItemKey};
use jagua_rs::geometry::DTransformation;
use jagua_rs::probs::bpp::entities::{
    BPInstance, BPLayoutType, BPPlacement, BPProblem, BPSolution, LayKey,
};
use jagua_rs::Instant;
use log::{debug, log};
use rand::prelude::SliceRandom;
use rand::rngs::Xoshiro256PlusPlus;
use slotmap::SecondaryMap;
use tap::Tap;

pub struct BPSeparator {
    pub instance: BPInstance,
    pub prob: BPProblem,
    /// One [`CollisionTracker`] per currently-open layout.
    pub cts: SecondaryMap<LayKey, CollisionTracker>,
    pub rng: Xoshiro256PlusPlus,
    pub config: SeparatorConfig,
}

/// Snapshot bundling problem + per-layout CT snapshots so a `restore` can be
/// rolled back exactly.
#[derive(Clone)]
pub struct BPSepSnapshot {
    pub sol: BPSolution,
    pub cts: SecondaryMap<LayKey, CTSnapshot>,
}

impl BPSeparator {
    pub fn new(
        instance: BPInstance,
        prob: BPProblem,
        rng: Xoshiro256PlusPlus,
        config: SeparatorConfig,
    ) -> Self {
        let cts = prob
            .layouts
            .iter()
            .map(|(k, l)| (k, CollisionTracker::new(l)))
            .collect();
        Self {
            instance,
            prob,
            cts,
            rng,
            config,
        }
    }

    /// Total loss summed over every open layout.
    pub fn total_loss(&self) -> f32 {
        self.cts.values().map(|ct| ct.get_total_loss()).sum()
    }

    /// Total weighted loss summed over every open layout (used for tie-breaking).
    pub fn total_weighted_loss(&self) -> f32 {
        self.cts
            .values()
            .map(|ct| ct.get_total_weighted_loss())
            .sum()
    }

    /// Capture a full snapshot of the problem + every layout's CT.
    pub fn save(&self) -> BPSepSnapshot {
        BPSepSnapshot {
            sol: self.prob.save(),
            cts: self.cts.iter().map(|(k, ct)| (k, ct.clone())).collect(),
        }
    }

    /// Restore from a [`BPSepSnapshot`]. The CTs are kept verbatim; if the
    /// problem's layout set changed since the snapshot was taken, missing CTs
    /// are rebuilt from scratch and stale ones are dropped.
    pub fn restore(&mut self, snap: &BPSepSnapshot) {
        self.prob.restore(&snap.sol);

        // Drop CTs for layouts that no longer exist.
        let live: Vec<LayKey> = self.prob.layouts.keys().collect();
        let stale: Vec<LayKey> = self
            .cts
            .keys()
            .filter(|k| !live.contains(k))
            .collect();
        for k in stale {
            self.cts.remove(k);
        }

        // Restore (or rebuild) a CT for every live layout.
        for k in live {
            match snap.cts.get(k) {
                Some(snap_ct) => {
                    self.cts.insert(k, snap_ct.clone());
                }
                None => {
                    self.cts
                        .insert(k, CollisionTracker::new(&self.prob.layouts[k]));
                }
            }
        }
    }

    /// Rebuild the CT for a single layout from scratch.
    pub fn rebuild_ct(&mut self, lkey: LayKey) {
        self.cts
            .insert(lkey, CollisionTracker::new(&self.prob.layouts[lkey]));
    }

    /// Run the separation algorithm on every open layout in turn until either
    /// (a) every layout reports zero loss, or (b) the strike limit is hit.
    ///
    /// Returns the best snapshot encountered.
    pub fn separate(
        &mut self,
        term: &impl Terminator,
        sol_listener: &mut impl SolutionListener<BPProblem>,
    ) -> BPSepSnapshot {
        let mut min_snap = self.save();
        let mut min_loss = self.total_loss();
        log!(
            self.config.log_level,
            "[BP-SEP] starting separation across {} layout(s), total loss: {}",
            self.prob.layouts.len(),
            FMT().fmt2(min_loss)
        );

        let mut n_strikes = 0;
        let mut n_iter = 0;
        let start = Instant::now();

        'outer: while n_strikes < self.config.strike_limit && !term.kill() {
            let initial_strike_loss = self.total_loss();
            let mut n_iter_no_improvement = 0;

            while n_iter_no_improvement < self.config.iter_no_imprv_limit && !term.kill() {
                let loss_before = self.total_loss();
                self.move_items_one_pass();
                let loss = self.total_loss();

                debug!(
                    "[BP-SEP] [s:{n_strikes},i:{n_iter}] l: {} -> {} (min l: {})",
                    FMT().fmt2(loss_before),
                    FMT().fmt2(loss),
                    FMT().fmt2(min_loss)
                );

                if loss == 0.0 {
                    log!(
                        self.config.log_level,
                        "[BP-SEP] [s:{n_strikes},i:{n_iter}] (S) all layouts feasible"
                    );
                    min_snap = self.save();
                    min_loss = 0.0;
                    break 'outer;
                } else if loss < min_loss {
                    log!(
                        self.config.log_level,
                        "[BP-SEP] [s:{n_strikes},i:{n_iter}] (*) min_l: {}",
                        FMT().fmt2(loss)
                    );
                    sol_listener.report(ReportType::ExplImproving, &self.prob.save(), &self.instance);
                    if loss < min_loss * 0.98 {
                        n_iter_no_improvement = 0;
                    }
                    min_snap = self.save();
                    min_loss = loss;
                } else {
                    n_iter_no_improvement += 1;
                }

                // Update the GLS weights in every layout.
                for ct in self.cts.values_mut() {
                    ct.update_weights();
                }
                n_iter += 1;
            }

            if initial_strike_loss * 0.98 <= min_loss {
                n_strikes += 1;
            } else {
                n_strikes = 0;
            }
            // Roll back to the best snapshot before starting the next strike.
            self.restore(&min_snap);
        }

        let secs = start.elapsed().as_secs_f32();
        log!(
            self.config.log_level,
            "[BP-SEP] finished in {:.3}s, iter: {}, final total loss: {}",
            secs,
            n_iter,
            FMT().fmt2(min_loss)
        );
        min_snap
    }

    /// One full sweep: visit every open layout (with ≥ 2 items) once and let
    /// every colliding item attempt one move.
    fn move_items_one_pass(&mut self) {
        let lkeys: Vec<LayKey> = self.prob.layouts.keys().collect();
        for lkey in lkeys {
            if self.prob.layouts[lkey].placed_items.len() < 2 {
                continue; // see "V1 limitations" in the module docstring
            }
            self.move_items_in_layout(lkey);
        }
    }

    /// Equivalent of [`crate::optimizer::spp::worker::SeparatorWorker::move_items`]
    /// but scoped to a single layout in a [`BPProblem`].
    fn move_items_in_layout(&mut self, lkey: LayKey) {
        let candidates = self.prob.layouts[lkey]
            .placed_items
            .keys()
            .filter(|pk| self.cts[lkey].get_loss(*pk) > 0.0)
            .collect_vec()
            .tap_mut(|v| v.shuffle(&mut self.rng));

        for &pk in candidates.iter() {
            // Re-check: the item might have stopped colliding after previous moves.
            if self.cts[lkey].get_loss(pk) <= 0.0 {
                continue;
            }
            // Defensive: a move could have closed the layout (shouldn't with
            // the ≥ 2 guard, but guard anyway).
            if !self.prob.layouts.contains_key(lkey) {
                return;
            }
            if !self.prob.layouts[lkey].placed_items.contains_key(pk) {
                continue;
            }

            let item_id = self.prob.layouts[lkey].placed_items[pk].item_id;
            let item = self.instance.item(item_id);
            let layout = &self.prob.layouts[lkey];
            let evaluator = SeparationEvaluator::new(layout, item, pk, &self.cts[lkey]);

            let (best_sample, _n_evals) = search::search_placement(
                layout,
                item,
                Some(pk),
                evaluator,
                self.config.sample_config,
                &mut self.rng,
            );

            let (new_dt, _) = best_sample.expect("search_placement should always return a sample");
            self.move_item_in_layout(lkey, pk, new_dt);
        }
    }

    /// Move a single item to a new transform within `lkey`'s layout. The
    /// caller must guarantee that the layout has ≥ 2 items so the
    /// remove-then-place sequence won't close-then-reopen the bin.
    pub fn move_item_in_layout(
        &mut self,
        lkey: LayKey,
        pk: PItemKey,
        new_dt: DTransformation,
    ) -> PItemKey {
        debug_assert!(tracker_matches_layout(&self.cts[lkey], &self.prob.layouts[lkey]));
        debug_assert!(
            self.prob.layouts[lkey].placed_items.len() >= 2,
            "intra-layout move requires layout to retain at least one other item"
        );

        let item_id = self.prob.layouts[lkey].placed_items[pk].item_id;
        let removed = self.prob.remove_item(lkey, pk);
        // The remove kept the layout open (≥ 2 items invariant), so its lkey
        // is still valid and we can re-place via Open(lkey).
        debug_assert!(self.prob.layouts.contains_key(lkey));

        let new_placement = BPPlacement {
            layout_id: BPLayoutType::Open(lkey),
            item_id,
            d_transf: new_dt,
        };
        let _ = removed; // (BPPlacement is `Copy`; we only used it to confirm semantics.)
        let (new_lkey, new_pk) = self.prob.place_item(new_placement);
        debug_assert_eq!(new_lkey, lkey);

        self.cts
            .get_mut(lkey)
            .unwrap()
            .register_item_move(&self.prob.layouts[lkey], pk, new_pk);

        debug_assert!(tracker_matches_layout(&self.cts[lkey], &self.prob.layouts[lkey]));
        new_pk
    }
}
