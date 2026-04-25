extern crate core;

use clap::Parser as Clap;
use jagua_rs::io::import::Importer;
use log::{info, warn, Level};
use rand::SeedableRng;
use sparrow::config::*;
use sparrow::optimizer::{optimize_bpp, optimize_spp};
use sparrow::util::io;
use sparrow::util::io::{ExtBPOutput, ExtSPOutput, LoadedInstance, MainCli};
use sparrow::EPOCH;
use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};
use rand::rngs::Xoshiro256PlusPlus;
use sparrow::consts::{DEFAULT_COMPRESS_TIME_RATIO, DEFAULT_EXPLORE_TIME_RATIO, DEFAULT_FAIL_DECAY_RATIO_CMPR, DEFAULT_MAX_CONSEQ_FAILS_EXPL, LOG_LEVEL_FILTER_DEBUG, LOG_LEVEL_FILTER_RELEASE};
use sparrow::util::ctrlc_terminator::CtrlCTerminator;
use sparrow::util::svg_exporter::SvgExporter;

pub const OUTPUT_DIR: &str = "output";

pub const LIVE_DIR: &str = "data/live";

fn main() -> Result<()>{
    let mut config = DEFAULT_SPARROW_CONFIG;

    fs::create_dir_all(OUTPUT_DIR)?;
    let log_file_path = format!("{}/log.txt", OUTPUT_DIR);
    match cfg!(debug_assertions) {
        true => io::init_logger(LOG_LEVEL_FILTER_DEBUG, Path::new(&log_file_path))?,
        false => io::init_logger(LOG_LEVEL_FILTER_RELEASE, Path::new(&log_file_path))?,
    }

    let args = MainCli::parse();
    let input_file_path = &args.input;
    let (explore_dur, compress_dur) = match (args.global_time, args.exploration, args.compression) {
        (Some(gt), None, None) => {
            (Duration::from_secs(gt).mul_f32(DEFAULT_EXPLORE_TIME_RATIO), Duration::from_secs(gt).mul_f32(DEFAULT_COMPRESS_TIME_RATIO))
        },
        (None, Some(et), Some(ct)) => {
            (Duration::from_secs(et), Duration::from_secs(ct))
        },
        (None, None, None) => {
            warn!("[MAIN] no time limit specified");
            (Duration::from_secs(600).mul_f32(DEFAULT_EXPLORE_TIME_RATIO), Duration::from_secs(600).mul_f32(DEFAULT_COMPRESS_TIME_RATIO))
        },
        _ => bail!("invalid cli pattern (clap should have caught this)"),
    };
    config.expl_cfg.time_limit = explore_dur;
    config.cmpr_cfg.time_limit = compress_dur;
    if args.early_termination {
        config.expl_cfg.max_conseq_failed_attempts = Some(DEFAULT_MAX_CONSEQ_FAILS_EXPL);
        config.cmpr_cfg.shrink_decay = ShrinkDecayStrategy::FailureBased(DEFAULT_FAIL_DECAY_RATIO_CMPR);
        warn!("[MAIN] early termination enabled!");
    }
    if let Some(arg_rng_seed) = args.rng_seed {
        config.rng_seed = Some(arg_rng_seed as usize);
    }
    if let Some(sep) = args.min_item_separation {
        if sep < 0.0 {
            bail!("--min-item-separation must be >= 0");
        }
        config.min_item_separation = if sep == 0.0 { None } else { Some(sep) };
        info!("[MAIN] min_item_separation = {}", sep);
    }
    if let Some(sep) = args.min_hole_separation {
        if sep < 0.0 {
            bail!("--min-hole-separation must be >= 0");
        }
        config.min_hole_separation = if sep == 0.0 { None } else { Some(sep) };
        info!("[MAIN] min_hole_separation = {}", sep);
    }

    info!("[MAIN] configured to explore for {}s and compress for {}s", explore_dur.as_secs(), compress_dur.as_secs());

    let rng = match config.rng_seed {
        Some(seed) => {
            info!("[MAIN] using seed: {}", seed);
            Xoshiro256PlusPlus::seed_from_u64(seed as u64)
        },
        None => {
            let seed = rand::random();
            warn!("[MAIN] no seed provided, using: {}", seed);
            Xoshiro256PlusPlus::seed_from_u64(seed)
        }
    };

    info!("[MAIN] system time: {}", jiff::Timestamp::now());

    let importer = Importer::new(
        config.cde_config,
        config.poly_simpl_tolerance,
        config.min_item_separation,
        config.min_hole_separation,
        config.narrow_concavity_cutoff_ratio,
    );

    let loaded = io::read_input(Path::new(&input_file_path))?;

    match loaded {
        LoadedInstance::Spp { ext_instance, ext_solution } => {
            run_spp(ext_instance, ext_solution, &importer, &config, rng)
        }
        LoadedInstance::Bpp { ext_instance } => {
            // For BPP, the SPP exploration/compression times are reused as a single
            // exploration budget (compression is not implemented for BPP yet).
            let mut bpp_config = config;
            bpp_config.bpp_expl_cfg.time_limit = explore_dur + compress_dur;
            run_bpp(ext_instance, &importer, &bpp_config, rng)
        }
    }
}

fn run_spp(
    ext_instance: jagua_rs::probs::spp::io::ext_repr::ExtSPInstance,
    ext_solution: Option<jagua_rs::probs::spp::io::ext_repr::ExtSPSolution>,
    importer: &Importer,
    config: &SparrowConfig,
    rng: Xoshiro256PlusPlus,
) -> Result<()> {
    let instance = jagua_rs::probs::spp::io::import_instance(importer, &ext_instance)?;

    let initial_solution = ext_solution.map(|e|
        jagua_rs::probs::spp::io::import_solution(&instance, &e)
    );

    info!("[MAIN] loaded SPP instance {} with #{} items", ext_instance.name, instance.total_item_qty());

    let mut svg_exporter = build_svg_exporter(&ext_instance.name);
    let mut ctrlc_terminator = CtrlCTerminator::new();

    let solution = optimize_spp(
        instance.clone(),
        rng,
        &mut svg_exporter,
        &mut ctrlc_terminator,
        &config.expl_cfg,
        &config.cmpr_cfg,
        initial_solution.as_ref(),
    );

    let json_path = format!("{OUTPUT_DIR}/final_{}.json", ext_instance.name);
    let json_output = ExtSPOutput {
        instance: ext_instance,
        solution: jagua_rs::probs::spp::io::export(&instance, &solution, *EPOCH),
    };
    io::write_json(&json_output, Path::new(json_path.as_str()), Level::Info)?;
    Ok(())
}

fn run_bpp(
    ext_instance: jagua_rs::probs::bpp::io::ext_repr::ExtBPInstance,
    importer: &Importer,
    config: &SparrowConfig,
    rng: Xoshiro256PlusPlus,
) -> Result<()> {
    let instance = jagua_rs::probs::bpp::io::import_instance(importer, &ext_instance)?;

    info!(
        "[MAIN] loaded BPP instance {} with #{} items, {} bin type(s)",
        ext_instance.name,
        instance.total_item_qty(),
        instance.bins.len()
    );

    let mut svg_exporter = build_svg_exporter(&ext_instance.name);
    let mut ctrlc_terminator = CtrlCTerminator::new();

    let solution = optimize_bpp(
        instance.clone(),
        rng,
        &mut svg_exporter,
        &mut ctrlc_terminator,
        &config.bpp_expl_cfg,
        config.bpp_sep_cfg,
    );

    let json_path = format!("{OUTPUT_DIR}/final_{}.json", ext_instance.name);
    let json_output = ExtBPOutput {
        instance: ext_instance,
        solution: jagua_rs::probs::bpp::io::export(&instance, &solution, *EPOCH),
    };
    io::write_json(&json_output, Path::new(json_path.as_str()), Level::Info)?;
    Ok(())
}

fn build_svg_exporter(name: &str) -> SvgExporter {
    let final_svg_path = Some(format!("{OUTPUT_DIR}/final_{}.svg", name));

    let intermediate_svg_dir = match cfg!(feature = "only_final_svg") {
        true => None,
        false => Some(format!("{OUTPUT_DIR}/sols_{}", name)),
    };

    let live_svg_path = match cfg!(feature = "live_svg") {
        true => Some(format!("{LIVE_DIR}/.live_solution.svg")),
        false => None,
    };

    SvgExporter::new(final_svg_path, intermediate_svg_dir, live_svg_path)
}
