use crate::config::*;
use crate::consts::LBF_SAMPLE_CONFIG;
use crate::optimizer::spp::compress::compression_phase;
use crate::optimizer::spp::explore::exploration_phase;
use crate::optimizer::spp::lbf::LBFBuilder;
use crate::optimizer::spp::separator::Separator;
use crate::util::listener::{ReportType, SolutionListener};
use crate::util::terminator::Terminator;
use jagua_rs::probs::spp::entities::{SPInstance, SPSolution};
use log::info;
use rand::{Rng, SeedableRng};
use std::time::Duration;
use rand::rngs::Xoshiro256PlusPlus;

pub mod problem;
pub mod spp;
pub mod bpp;

/// Strip Packing Problem optimizer.
///
/// Algorithm 11 from <https://doi.org/10.48550/arXiv.2509.13329>.
pub fn optimize_spp(
    instance: SPInstance,
    mut rng: Xoshiro256PlusPlus,
    sol_listener: &mut impl SolutionListener<jagua_rs::probs::spp::entities::SPProblem>,
    terminator: &mut impl Terminator,
    expl_config: &ExplorationConfig,
    cmpr_config: &CompressionConfig,
    initial_solution: Option<&SPSolution>
) -> SPSolution {
    let mut next_rng = || Xoshiro256PlusPlus::seed_from_u64(rng.next_u64());
    
    // First build an initial solution if none is provided
    let start_prob = match initial_solution {
        None => {
            let builder = LBFBuilder::new(instance.clone(), next_rng(), LBF_SAMPLE_CONFIG).construct();
            builder.prob
        }
        Some(init_sol) => {
            info!("[OPT] warm starting from provided initial solution");
            let mut prob = jagua_rs::probs::spp::entities::SPProblem::new(instance.clone());
            prob.restore(init_sol);
            prob
        }
    };

    // Begin by executing the exploration phase
    terminator.new_timeout(expl_config.time_limit);
    let mut expl_separator = Separator::new(instance.clone(), start_prob, next_rng(), expl_config.separator_config);
    let solutions = exploration_phase(
        &instance,
        &mut expl_separator,
        sol_listener,
        terminator,
        expl_config,
    );
    let final_explore_sol = solutions.last().unwrap().clone();

    // Start the compression phase from the final solution from the exploration phase
    terminator.new_timeout(cmpr_config.time_limit);
    let mut cmpr_separator = Separator::new(expl_separator.instance, expl_separator.prob, next_rng(), cmpr_config.separator_config);
    let cmpr_sol = compression_phase(
        &instance,
        &mut cmpr_separator,
        &final_explore_sol,
        sol_listener,
        terminator,
        cmpr_config,
    );

    sol_listener.report(ReportType::Final, &cmpr_sol, &instance);

    // Return the final compressed solution
    cmpr_sol
}

/// Bin Packing Problem optimizer.
///
/// V1 pipeline: LBF construction → bin-removal exploration. The compression
/// phase has no direct BPP analog yet (strip-shrink is SPP-specific) and is
/// deferred to a later iteration.
pub fn optimize_bpp(
    instance: jagua_rs::probs::bpp::entities::BPInstance,
    mut rng: Xoshiro256PlusPlus,
    sol_listener: &mut impl SolutionListener<jagua_rs::probs::bpp::entities::BPProblem>,
    terminator: &mut impl Terminator,
    expl_config: &crate::optimizer::bpp::explore::BPExplorationConfig,
    sep_config: crate::optimizer::spp::separator::SeparatorConfig,
) -> jagua_rs::probs::bpp::entities::BPSolution {
    use crate::optimizer::bpp::explore::bpp_exploration;
    use crate::optimizer::bpp::lbf::BPLBFBuilder;
    use crate::optimizer::bpp::separator::BPSeparator;

    let mut next_rng = || Xoshiro256PlusPlus::seed_from_u64(rng.next_u64());

    // 1. LBF construction.
    info!("[OPT-BPP] constructing initial solution via LBF");
    let lbf_builder = BPLBFBuilder::new(instance.clone(), next_rng(), LBF_SAMPLE_CONFIG)
        .construct()
        .expect("BPP LBF construction failed");
    let start_prob = lbf_builder.prob;
    info!(
        "[OPT-BPP] LBF placed {} item(s) into {} bin(s)",
        start_prob.n_placed_items(),
        start_prob.layouts.len()
    );

    // 2. Exploration: bin-removal loop.
    terminator.new_timeout(expl_config.time_limit);
    let mut sep = BPSeparator::new(instance.clone(), start_prob, next_rng(), sep_config);
    let solutions = bpp_exploration(&mut sep, sol_listener, terminator, expl_config);
    let final_sol = solutions.last().expect("at least the initial solution is feasible").clone();

    sol_listener.report(ReportType::Final, &final_sol, &instance);
    final_sol
}