//! Bin-removal exploration phase for the Bin Packing Problem.
//!
//! High-level loop: pick the **least-loaded open bin** (lowest area density),
//! free its items, redistribute them into the remaining open bins (allowing
//! initial overlap), then run the [`BPSeparator`] to resolve the introduced
//! overlaps. If separation succeeds → permanently fewer bins; if it fails →
//! roll back and try a different bin (or terminate).
//!
//! V1 scope:
//! - **Best-fit injection**: each freed item is dropped into the open layout
//!   with the most remaining free area, at a sample chosen by an LBF-style
//!   placement search (which may find a Clear position; if not, we fall back
//!   to placing at the layout's centroid → the separator then resolves the
//!   overlap).
//! - **No diversification / restart pool** (the SPP exploration's infeasible
//!   solution pool + disrupt step has no obvious BPP analog yet).

use crate::eval::lbf_evaluator::LBFEvaluator;
use crate::eval::sample_eval::SampleEval;
use crate::optimizer::bpp::separator::{BPSepSnapshot, BPSeparator};
use crate::sample::search;
use crate::util::listener::{ReportType, SolutionListener};
use crate::util::terminator::Terminator;
use itertools::Itertools;
use jagua_rs::entities::Instance;
use jagua_rs::geometry::DTransformation;
use jagua_rs::probs::bpp::entities::{BPLayoutType, BPPlacement, BPProblem, BPSolution, LayKey};
use log::info;
use ordered_float::OrderedFloat;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct BPExplorationConfig {
    /// Hard cap on the number of bin-removal attempts.
    pub max_bin_removal_attempts: usize,
    /// Wall-clock budget for the whole exploration phase.
    pub time_limit: Duration,
}

/// Run bin-removal exploration. Returns every feasible solution encountered
/// (in chronological order; the last entry is the best — i.e. fewest bins).
pub fn bpp_exploration(
    sep: &mut BPSeparator,
    sol_listener: &mut impl SolutionListener<BPProblem>,
    term: &impl Terminator,
    config: &BPExplorationConfig,
) -> Vec<BPSolution> {
    // Sanity: separator must start from a feasible (zero-loss) state.
    let initial_loss = sep.total_loss();
    assert_eq!(
        initial_loss, 0.0,
        "bpp_exploration requires a feasible initial layout (loss = {})",
        initial_loss
    );

    let mut feasible_sols = vec![sep.prob.save()];
    let initial_n_bins = sep.prob.layouts.len();
    sol_listener.report(
        ReportType::ExplFeas,
        feasible_sols.last().unwrap(),
        &sep.instance,
    );
    info!(
        "[BP-EXPL] starting with {} bin(s), density {:.3}%",
        initial_n_bins,
        sep.prob.density() * 100.0
    );

    // Track which bins to skip in subsequent attempts (those that failed to
    // be removed already). Resets whenever any removal succeeds, since the
    // layout topology changes.
    let mut blacklist: Vec<LayKey> = vec![];

    for attempt in 0..config.max_bin_removal_attempts {
        if term.kill() {
            info!("[BP-EXPL] terminated by signal");
            break;
        }
        if sep.prob.layouts.len() <= 1 {
            info!("[BP-EXPL] cannot remove the only remaining bin");
            break;
        }

        // Pick the least-loaded open bin (lowest area density) that is not
        // blacklisted.
        let target = sep
            .prob
            .layouts
            .iter()
            .filter(|(k, _)| !blacklist.contains(k))
            .min_by_key(|(_, l)| {
                let density = l.placed_item_area(&sep.instance) / l.container.area();
                OrderedFloat(density)
            })
            .map(|(k, _)| k);

        let target_lkey = match target {
            Some(k) => k,
            None => {
                info!("[BP-EXPL] no removable bins left (all {} blacklisted)", blacklist.len());
                break;
            }
        };

        let pre_snap = sep.save();
        let pre_n_bins = sep.prob.layouts.len();
        info!(
            "[BP-EXPL] attempt {}/{}: trying to remove layout {:?} ({} -> {} bins)",
            attempt + 1,
            config.max_bin_removal_attempts,
            target_lkey,
            pre_n_bins,
            pre_n_bins - 1
        );

        let success = try_remove_bin(sep, target_lkey);

        if success {
            // Run the separator to resolve any overlaps introduced by injection.
            let result_snap = sep.separate(term, sol_listener);
            if result_snap.sol.layout_snapshots.len() < pre_n_bins
                && total_loss_of_snap(&result_snap) == 0.0
            {
                // Bin removed and feasibility preserved.
                let new_n_bins = result_snap.sol.layout_snapshots.len();
                info!(
                    "[BP-EXPL] (✓) removed a bin: {} -> {} bins, density {:.3}%",
                    pre_n_bins,
                    new_n_bins,
                    sep.prob.density() * 100.0
                );
                feasible_sols.push(result_snap.sol.clone());
                sol_listener.report(ReportType::ExplFeas, &result_snap.sol, &sep.instance);
                blacklist.clear();
                continue;
            }
        }

        // Failed: roll back and blacklist this layout for the next attempt.
        info!(
            "[BP-EXPL] (✗) attempt {} failed; rolling back",
            attempt + 1
        );
        sep.restore(&pre_snap);
        blacklist.push(target_lkey);
    }

    info!(
        "[BP-EXPL] finished: {} bin(s) ({} bin(s) removed), density {:.3}%",
        sep.prob.layouts.len(),
        initial_n_bins - sep.prob.layouts.len(),
        sep.prob.density() * 100.0
    );
    feasible_sols
}

fn total_loss_of_snap(snap: &BPSepSnapshot) -> f32 {
    snap.cts.values().map(|ct| ct.get_total_loss()).sum()
}

/// Free `target_lkey`'s items and re-inject them into the remaining open
/// bins. Returns `false` if any item could not be placed at all (no remaining
/// bin had enough free area in its bbox to host the item even with overlap).
fn try_remove_bin(sep: &mut BPSeparator, target_lkey: LayKey) -> bool {
    // Snapshot the items that need to be relocated.
    let freed: Vec<usize> = sep.prob.layouts[target_lkey]
        .placed_items
        .values()
        .map(|pi| pi.item_id)
        .collect();

    // Close the target bin by removing it via the public API.
    sep.prob.remove_layout(target_lkey);
    sep.cts.remove(target_lkey);

    // Re-insert each freed item, sorted by area desc (largest first → best
    // chance of finding a clear spot before overlaps accumulate).
    let sorted = freed
        .into_iter()
        .sorted_by_key(|id| OrderedFloat(-sep.instance.item(*id).shape_cd.area))
        .collect_vec();

    for item_id in sorted {
        if !inject_item(sep, item_id) {
            return false;
        }
    }
    true
}

/// Inject one item into the open bin with the most free area. Tries an LBF
/// search first; if that finds no Clear position, places at the container's
/// centroid and lets the separator deal with the overlap.
fn inject_item(sep: &mut BPSeparator, item_id: usize) -> bool {
    if sep.prob.layouts.is_empty() {
        return false;
    }

    // Pick the best-fit (most free area) open layout.
    let target_lkey = sep
        .prob
        .layouts
        .iter()
        .max_by_key(|(_, l)| {
            let free = l.container.area() - l.placed_item_area(&sep.instance);
            OrderedFloat(free)
        })
        .map(|(k, _)| k)
        .expect("at least one open layout exists");

    let item = sep.instance.item(item_id);
    let layout = &sep.prob.layouts[target_lkey];

    // Try an LBF placement first.
    let lbf_evaluator = LBFEvaluator::new(layout, item);
    let (best_sample, _) = search::search_placement(
        layout,
        item,
        None,
        lbf_evaluator,
        sep.config.sample_config,
        &mut sep.rng,
    );

    let d_transf: DTransformation = match best_sample {
        Some((dt, SampleEval::Clear { .. })) => dt,
        _ => {
            // Fall back: place at the container's bbox centre (will collide;
            // separator resolves later).
            let bbox = layout.container.outer_cd.bbox;
            DTransformation::new(
                0.0,
                (
                    bbox.x_min + bbox.width() / 2.0,
                    bbox.y_min + bbox.height() / 2.0,
                ),
            )
        }
    };

    sep.prob.place_item(BPPlacement {
        layout_id: BPLayoutType::Open(target_lkey),
        item_id,
        d_transf,
    });
    // Rebuild the CT for the modified layout from scratch.
    sep.rebuild_ct(target_lkey);
    true
}
