//! Strip Packing Problem (SPP) optimizer.
//!
//! All modules in this directory are SPP-specific: they rely on
//! `jagua_rs::probs::spp` types and on strip-resize semantics
//! (`change_strip_width`, `fit_strip`, etc.). The Bin Packing Problem
//! variant lives in a parallel `optimizer::bpp` module (added in Stage 3+
//! of the BPP plan).

pub mod compress;
pub mod explore;
pub mod lbf;
pub mod separator;
pub(crate) mod worker;
