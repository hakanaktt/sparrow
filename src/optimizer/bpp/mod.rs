//! Bin Packing Problem (BPP) optimizer.
//!
//! Mirrors the structure of [`super::spp`] but operates on `BPProblem`,
//! `BPSolution`, etc. Algorithms differ where the problem semantics differ
//! (notably: opening/closing bins instead of resizing a strip).

pub mod lbf;
