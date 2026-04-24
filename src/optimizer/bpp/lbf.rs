//! Left-Bottom-Fill construction heuristic for Bin Packing.
//!
//! Strategy (First-Fit Decreasing variant):
//!   1. Sort items by `convex_hull_area * diameter` descending (same key as
//!      the SPP LBF builder).
//!   2. For each item, attempt to place it in each currently-open bin, in
//!      ascending order of free area (i.e. tightest fit first). The placement
//!      search uses [`LBFEvaluator`] which only accepts collision-free
//!      positions and rewards left-bottom proximity.
//!   3. If no open bin accepts the item, open a new bin of the cheapest
//!      available type whose container's bounding box can fit the item, and
//!      place the item there using a search against an empty layout.
//!   4. If no bin type can fit the item at all, return an error.

use crate::eval::lbf_evaluator::LBFEvaluator;
use crate::eval::sample_eval::SampleEval;
use crate::sample::search::{search_placement, SampleConfig};
use itertools::Itertools;
use jagua_rs::entities::{Instance, Layout};
use jagua_rs::geometry::DTransformation;
use jagua_rs::probs::bpp::entities::{
    BPInstance, BPLayoutType, BPPlacement, BPProblem, LayKey,
};
use jagua_rs::Instant;
use log::{debug, info};
use ordered_float::OrderedFloat;
use rand::rngs::Xoshiro256PlusPlus;
use std::cmp::Reverse;
use std::iter;

pub struct BPLBFBuilder {
    pub instance: BPInstance,
    pub prob: BPProblem,
    pub rng: Xoshiro256PlusPlus,
    pub sample_config: SampleConfig,
}

#[derive(Debug)]
pub enum BPLBFError {
    /// No bin type with available stock can hold the item.
    ItemDoesNotFitAnyBin { item_id: usize },
    /// All bin types are out of stock and the item cannot be placed in any
    /// currently open bin.
    OutOfBinStock { item_id: usize },
}

impl BPLBFBuilder {
    pub fn new(instance: BPInstance, rng: Xoshiro256PlusPlus, sample_config: SampleConfig) -> Self {
        let prob = BPProblem::new(instance.clone());
        Self {
            instance,
            prob,
            rng,
            sample_config,
        }
    }

    pub fn construct(mut self) -> Result<Self, BPLBFError> {
        let start = Instant::now();
        let n_items = self.instance.items.len();

        let sorted_item_indices = (0..n_items)
            .sorted_by_cached_key(|id| {
                let item_shape = self.instance.item(*id).shape_cd.as_ref();
                let convex_hull_area = item_shape.surrogate().convex_hull_area;
                let diameter = item_shape.diameter;
                Reverse(OrderedFloat(convex_hull_area * diameter))
            })
            .flat_map(|id| {
                let missing_qty = self.prob.item_demand_qtys[id];
                iter::repeat_n(id, missing_qty)
            })
            .collect_vec();

        debug!("[BPP-CONSTR] placing items in order: {:?}", sorted_item_indices);

        for item_id in sorted_item_indices {
            self.place_item(item_id)?;
        }

        info!(
            "[BPP-CONSTR] placed all {} items into {} bin(s) (in {:?})",
            self.prob.n_placed_items(),
            self.prob.layouts.len(),
            start.elapsed()
        );
        Ok(self)
    }

    fn place_item(&mut self, item_id: usize) -> Result<(), BPLBFError> {
        // 1. Try open bins, tightest fit (lowest free area) first.
        let candidate_keys: Vec<LayKey> = {
            let item_area = self.instance.item(item_id).shape_cd.area;
            let mut keys: Vec<(LayKey, f32)> = self
                .prob
                .layouts
                .iter()
                .filter_map(|(k, l)| {
                    let free = l.container.area() - l.placed_item_area(&self.instance);
                    if free >= item_area {
                        Some((k, free))
                    } else {
                        None
                    }
                })
                .collect();
            keys.sort_by_key(|(_, free)| OrderedFloat(*free));
            keys.into_iter().map(|(k, _)| k).collect()
        };

        for lkey in candidate_keys {
            if let Some(d_transf) = self.find_placement_in_existing_layout(lkey, item_id) {
                self.prob.place_item(BPPlacement {
                    layout_id: BPLayoutType::Open(lkey),
                    item_id,
                    d_transf,
                });
                debug!(
                    "[BPP-CONSTR] placed item {} into existing layout {:?}",
                    item_id, lkey
                );
                return Ok(());
            }
        }

        // 2. No open bin worked — open a new bin of the cheapest type with stock.
        let new_bin_id = self
            .prob
            .bin_stock_qtys
            .iter()
            .enumerate()
            .filter(|(_, stock)| **stock > 0)
            .min_by_key(|(bin_id, _)| self.instance.bins[*bin_id].cost)
            .map(|(bin_id, _)| bin_id);

        let bin_id = match new_bin_id {
            Some(id) => id,
            None => return Err(BPLBFError::OutOfBinStock { item_id }),
        };

        // Search for a placement against a *temporary* empty layout built from
        // the chosen bin's container. This avoids any chicken-and-egg with
        // BPProblem::place_item, which always inserts the item when opening a
        // bin via `Closed { bin_id }`.
        let scratch_layout = Layout::new(self.instance.bins[bin_id].container.clone());
        let item = self.instance.item(item_id);
        let evaluator = LBFEvaluator::new(&scratch_layout, item);
        let (best_sample, _) = search_placement(
            &scratch_layout,
            item,
            None,
            evaluator,
            self.sample_config,
            &mut self.rng,
        );
        let d_transf = match best_sample {
            Some((dt, SampleEval::Clear { .. })) => dt,
            _ => return Err(BPLBFError::ItemDoesNotFitAnyBin { item_id }),
        };

        // 3. Place the item — this opens the new bin atomically.
        self.prob.place_item(BPPlacement {
            layout_id: BPLayoutType::Closed { bin_id },
            item_id,
            d_transf,
        });
        debug!(
            "[BPP-CONSTR] opened new bin (type {}) for item {}",
            bin_id, item_id
        );
        Ok(())
    }

    fn find_placement_in_existing_layout(
        &mut self,
        lkey: LayKey,
        item_id: usize,
    ) -> Option<DTransformation> {
        let layout = &self.prob.layouts[lkey];
        let item = self.instance.item(item_id);
        let evaluator = LBFEvaluator::new(layout, item);

        let (best_sample, _) = search_placement(
            layout,
            item,
            None,
            evaluator,
            self.sample_config,
            &mut self.rng,
        );

        match best_sample {
            Some((dt, SampleEval::Clear { .. })) => Some(dt),
            _ => None,
        }
    }
}
