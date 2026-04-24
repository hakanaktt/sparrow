//! Stage 1 of the BPP plan (see `BPP_PLAN.md`): problem-type abstraction.
//!
//! These traits decouple the optimizer from `jagua_rs::probs::spp` so that a
//! `BPProblem` implementation can later be plugged in without changing the
//! algorithmic core. **No consumer changes are made in this stage** — the
//! existing optimizer still uses `SPProblem` directly. The traits only need to
//! exist and compile.
//!
//! Design notes:
//!
//! * `LayoutKey` is associated because SPP has exactly one layout (unit key,
//!   `()`) while BPP keys layouts by `slotmap::Key` (`jagua_rs::probs::bpp::LayKey`).
//! * `place_item` returns `(LayoutKey, PItemKey)` for both: SPP's `((), pik)`
//!   degenerates trivially; BPP returns the actual `(LayKey, PItemKey)`.
//! * `StripCapacity` and `BinCapacity` are mutually exclusive marker-extension
//!   traits — phase-level algorithms (exploration / compression) will be
//!   monomorphized against the appropriate one in Stage 4.
//! * No `change_strip_width(_, split)` overload here; the split-position
//!   variant lives on the `Separator`, not on the problem (`SPProblem` itself
//!   only exposes single-arg `change_strip_width`).

use jagua_rs::entities::{Layout, PItemKey};
use jagua_rs::probs::bpp::entities::{
    BPInstance, BPPlacement, BPProblem, BPSolution, LayKey,
};
use jagua_rs::probs::spp::entities::{SPInstance, SPPlacement, SPProblem, SPSolution};

/// Generic packing-problem interface shared by SPP and BPP.
///
/// All methods that the algorithmic core needs (placement, removal,
/// snapshot/restore, density) are expressed in terms of an abstract layout
/// key. Strip- or bin-specific capacity manipulation lives in the
/// [`StripCapacity`] and [`BinCapacity`] sub-traits.
pub trait PackingProblem {
    /// Static description of the problem (items, containers, demands).
    type Instance;
    /// Snapshot type produced by [`save`](Self::save) and consumed by [`restore`](Self::restore).
    type Solution: Clone;
    /// Placement descriptor consumed by [`place_item`](Self::place_item).
    type Placement: Copy;
    /// Identifier for a layout inside this problem.
    ///
    /// * SPP: `()` (always exactly one layout).
    /// * BPP: `jagua_rs::probs::bpp::LayKey` (slotmap key).
    type LayoutKey: Copy + Eq;

    fn instance(&self) -> &Self::Instance;

    /// Number of currently open layouts. Always `1` for SPP.
    fn n_layouts(&self) -> usize;

    /// Iterator over all currently open layout keys.
    fn layout_keys(&self) -> Box<dyn Iterator<Item = Self::LayoutKey> + '_>;

    fn layout(&self, key: Self::LayoutKey) -> &Layout;
    fn layout_mut(&mut self, key: Self::LayoutKey) -> &mut Layout;

    /// Place an item. Returns the layout it was placed in and its key inside
    /// that layout.
    fn place_item(&mut self, placement: Self::Placement) -> (Self::LayoutKey, PItemKey);

    /// Remove a previously placed item. Returns a placement that, if fed back
    /// into [`place_item`](Self::place_item), would restore the item.
    fn remove_item(&mut self, key: Self::LayoutKey, pik: PItemKey) -> Self::Placement;

    fn save(&self) -> Self::Solution;
    fn restore(&mut self, solution: &Self::Solution);

    /// Total placed-item area divided by total container area.
    fn density(&self) -> f32;
    fn n_placed_items(&self) -> usize;
}

/// Capacity model for strip-packing problems: a single resizable strip.
pub trait StripCapacity: PackingProblem {
    fn strip_width(&self) -> f32;
    fn change_strip_width(&mut self, new_width: f32);
    /// Shrink the strip to the minimum width that contains all placed items.
    fn fit_strip(&mut self);
}

/// Capacity model for bin-packing problems: variable number of fixed-size bins.
///
/// Implementors will be filled in during Stage 3 (BPP problem impl).
pub trait BinCapacity: PackingProblem {
    /// Number of currently open bins (== number of layouts).
    fn n_bins_used(&self) -> usize;

    /// Close a layout. The layout must be empty; otherwise behavior is
    /// implementation-defined (may panic).
    fn close_layout(&mut self, key: Self::LayoutKey);
}

// ----------------------------------------------------------------------------
// SPProblem implementation
// ----------------------------------------------------------------------------

impl PackingProblem for SPProblem {
    type Instance = SPInstance;
    type Solution = SPSolution;
    type Placement = SPPlacement;
    type LayoutKey = ();

    #[inline]
    fn instance(&self) -> &SPInstance {
        &self.instance
    }

    #[inline]
    fn n_layouts(&self) -> usize {
        1
    }

    fn layout_keys(&self) -> Box<dyn Iterator<Item = ()> + '_> {
        Box::new(std::iter::once(()))
    }

    #[inline]
    fn layout(&self, _key: ()) -> &Layout {
        &self.layout
    }

    #[inline]
    fn layout_mut(&mut self, _key: ()) -> &mut Layout {
        &mut self.layout
    }

    fn place_item(&mut self, placement: SPPlacement) -> ((), PItemKey) {
        let pik = SPProblem::place_item(self, placement);
        ((), pik)
    }

    fn remove_item(&mut self, _key: (), pik: PItemKey) -> SPPlacement {
        SPProblem::remove_item(self, pik)
    }

    fn save(&self) -> SPSolution {
        SPProblem::save(self)
    }

    fn restore(&mut self, solution: &SPSolution) {
        SPProblem::restore(self, solution)
    }

    fn density(&self) -> f32 {
        SPProblem::density(self)
    }

    fn n_placed_items(&self) -> usize {
        SPProblem::n_placed_items(self)
    }
}

impl StripCapacity for SPProblem {
    #[inline]
    fn strip_width(&self) -> f32 {
        SPProblem::strip_width(self)
    }

    #[inline]
    fn change_strip_width(&mut self, new_width: f32) {
        SPProblem::change_strip_width(self, new_width)
    }

    #[inline]
    fn fit_strip(&mut self) {
        SPProblem::fit_strip(self)
    }
}

// ----------------------------------------------------------------------------
// BPProblem implementation
// ----------------------------------------------------------------------------

impl PackingProblem for BPProblem {
    type Instance = BPInstance;
    type Solution = BPSolution;
    type Placement = BPPlacement;
    type LayoutKey = LayKey;

    #[inline]
    fn instance(&self) -> &BPInstance {
        &self.instance
    }

    #[inline]
    fn n_layouts(&self) -> usize {
        self.layouts.len()
    }

    fn layout_keys(&self) -> Box<dyn Iterator<Item = LayKey> + '_> {
        Box::new(self.layouts.keys())
    }

    #[inline]
    fn layout(&self, key: LayKey) -> &Layout {
        &self.layouts[key]
    }

    #[inline]
    fn layout_mut(&mut self, key: LayKey) -> &mut Layout {
        &mut self.layouts[key]
    }

    fn place_item(&mut self, placement: BPPlacement) -> (LayKey, PItemKey) {
        BPProblem::place_item(self, placement)
    }

    fn remove_item(&mut self, key: LayKey, pik: PItemKey) -> BPPlacement {
        BPProblem::remove_item(self, key, pik)
    }

    fn save(&self) -> BPSolution {
        BPProblem::save(self)
    }

    fn restore(&mut self, solution: &BPSolution) {
        BPProblem::restore(self, solution);
    }

    fn density(&self) -> f32 {
        BPProblem::density(self)
    }

    fn n_placed_items(&self) -> usize {
        BPProblem::n_placed_items(self)
    }
}

impl BinCapacity for BPProblem {
    #[inline]
    fn n_bins_used(&self) -> usize {
        self.layouts.len()
    }

    #[inline]
    fn close_layout(&mut self, key: LayKey) {
        BPProblem::remove_layout(self, key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jagua_rs::geometry::DTransformation;
    use jagua_rs::io::import::Importer;
    use jagua_rs::probs::spp::io::ext_repr::ExtSPInstance;
    use std::fs;

    /// Smoke test: drive `SPProblem` through the trait API only and verify the
    /// observable behavior matches the inherent-method baseline.
    #[test]
    fn spp_satisfies_packing_problem_trait() {
        let raw = fs::read_to_string("data/input/swim.json").unwrap();
        let ext: ExtSPInstance = serde_json::from_str(&raw).unwrap();
        let cfg = &crate::config::DEFAULT_SPARROW_CONFIG;
        let importer = Importer::new(
            cfg.cde_config,
            cfg.poly_simpl_tolerance,
            cfg.min_item_separation,
            cfg.narrow_concavity_cutoff_ratio,
        );
        let instance = jagua_rs::probs::spp::io::import_instance(&importer, &ext).unwrap();
        let mut prob = SPProblem::new(instance);

        // Trait surface
        assert_eq!(<SPProblem as PackingProblem>::n_layouts(&prob), 1);
        assert_eq!(prob.layout_keys().count(), 1);
        let _ = <SPProblem as PackingProblem>::layout(&prob, ());

        let initial_width = <SPProblem as StripCapacity>::strip_width(&prob);
        <SPProblem as StripCapacity>::change_strip_width(&mut prob, initial_width * 1.5);
        assert!((<SPProblem as StripCapacity>::strip_width(&prob) - initial_width * 1.5).abs() < 1e-3);

        // Place + remove via trait
        let placement = SPPlacement {
            item_id: 0,
            d_transf: DTransformation::empty(),
        };
        let ((), pik) = <SPProblem as PackingProblem>::place_item(&mut prob, placement);
        assert_eq!(<SPProblem as PackingProblem>::n_placed_items(&prob), 1);

        let snap = <SPProblem as PackingProblem>::save(&prob);
        let _back = <SPProblem as PackingProblem>::remove_item(&mut prob, (), pik);
        assert_eq!(<SPProblem as PackingProblem>::n_placed_items(&prob), 0);

        <SPProblem as PackingProblem>::restore(&mut prob, &snap);
        assert_eq!(<SPProblem as PackingProblem>::n_placed_items(&prob), 1);
    }
}
