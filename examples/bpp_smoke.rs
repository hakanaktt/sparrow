//! Stage 0 smoke test for the jagua-rs BPP API.
//!
//! Verifies that we can:
//! 1. Build a `BPInstance` (by reusing an SPP JSON's items & container).
//! 2. Construct a `BPProblem`, place an item (auto-opens a bin), `save()`,
//!    remove the item (auto-closes the empty bin), and `restore()` from snapshot.
//!
//! NOTE: `jagua_rs::probs::bpp::io::import_solution` is `unimplemented!()` in
//! jagua-rs 0.7.1, so warm-starting from a BPP solution JSON is not yet possible
//! (see Stage 0 findings in BPP_PLAN.md).
//!
//! Run with: `cargo run --release --example bpp_smoke`

use jagua_rs::geometry::DTransformation;
use jagua_rs::io::import::Importer;
use jagua_rs::probs::bpp::entities::{BPInstance, BPLayoutType, BPPlacement, BPProblem, Bin};
use jagua_rs::probs::spp::io::ext_repr::ExtSPInstance;
use sparrow::config::DEFAULT_SPARROW_CONFIG;
use std::fs;

fn main() -> anyhow::Result<()> {
    // ---- 1. Reshape an existing SPP instance into a BPInstance ----
    let raw = fs::read_to_string("data/input/swim.json")?;
    let ext_spp: ExtSPInstance = serde_json::from_str(&raw)?;

    let cfg = &DEFAULT_SPARROW_CONFIG;
    let importer = Importer::new(
        cfg.cde_config,
        cfg.poly_simpl_tolerance,
        cfg.min_item_separation,
        cfg.narrow_concavity_cutoff_ratio,
    );
    let spp_instance = jagua_rs::probs::spp::io::import_instance(&importer, &ext_spp)?;

    // SPInstance::containers() is empty for SPP — the strip becomes a Container only
    // inside SPProblem's Layout. Grab it from there.
    let spp_prob = jagua_rs::probs::spp::entities::SPProblem::new(spp_instance.clone());
    let mut strip_container = spp_prob.layout.container.clone();
    // BPP requires container ids to start at 0 and be consecutive — the strip's
    // container id may be anything (e.g. arbitrary), so normalize it.
    strip_container.id = 0;

    // Single bin type, stock = number of items (loose upper bound), zero cost.
    let bins = vec![Bin::new(strip_container, spp_instance.total_item_qty(), 0)];
    let items: Vec<_> = spp_instance.items.iter().cloned().collect();
    let bpp_instance = BPInstance::new(items, bins);

    println!(
        "[smoke] BPInstance: {} item types, {} bin types, total demand = {}",
        bpp_instance.items.len(),
        bpp_instance.bins.len(),
        bpp_instance.total_item_qty()
    );

    // ---- 2. Exercise BPProblem ----
    let mut prob = BPProblem::new(bpp_instance);
    assert_eq!(prob.layouts.len(), 0, "fresh BPProblem has no open layouts");

    // Place item 0 with identity transform — Closed{bin_id:0} auto-opens a bin.
    let (lkey, pik) = prob.place_item(BPPlacement {
        layout_id: BPLayoutType::Closed { bin_id: 0 },
        item_id: 0,
        d_transf: DTransformation::empty(),
    });
    assert_eq!(prob.layouts.len(), 1, "placing into Closed opens a bin");
    println!("[smoke] placed item 0 -> layout {:?}, pi {:?}", lkey, pik);

    let snap = prob.save();
    assert_eq!(snap.layout_snapshots.len(), 1);
    println!(
        "[smoke] save() captured {} layout(s)",
        snap.layout_snapshots.len()
    );

    let _removed = prob.remove_item(lkey, pik);
    assert_eq!(
        prob.layouts.len(),
        0,
        "removing the last item closes the layout"
    );

    let keys_changed = prob.restore(&snap);
    assert!(keys_changed, "restore reopens the closed layout");
    assert_eq!(prob.layouts.len(), 1);
    assert_eq!(prob.n_placed_items(), 1);
    println!("[smoke] restore() round-tripped successfully");

    println!("[smoke] OK");
    Ok(())
}
