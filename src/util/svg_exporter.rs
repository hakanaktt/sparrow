use crate::consts::DRAW_OPTIONS;
use crate::util::io;
use crate::util::listener::{ReportType, SolutionListener};
use jagua_rs::io::svg::s_layout_to_svg;
use jagua_rs::probs::bpp::entities::{BPInstance, BPProblem, BPSolution};
use jagua_rs::probs::spp::entities::{SPInstance, SPProblem, SPSolution};
use log::Level;
use std::path::Path;
pub struct SvgExporter {
    svg_counter: usize,
    /// Path to write the final SVG file to, if provided
    pub final_path: Option<String>,
    /// Directory to write all intermedia solution SVG files to, if provided
    pub intermediate_dir: Option<String>,
    /// Path to write the live SVG file to, if provided
    pub live_path: Option<String>,
}

impl SvgExporter {
    pub fn new(final_path: Option<String>, intermediate_dir: Option<String>, live_path: Option<String>) -> Self {
        // Clean all svg files from the intermediate directory if it is provided
        if let Some(intermediate_dir) = &intermediate_dir
            && let Ok(files_in_dir) = std::fs::read_dir(Path::new(intermediate_dir)) {
                for file in files_in_dir.flatten() {
                    if file.path().extension().unwrap_or_default() == "svg" {
                        std::fs::remove_file(file.path()).unwrap();
                    }
                }
            }
        
        SvgExporter {
            svg_counter: 0,
            final_path,
            intermediate_dir,
            live_path,
        }
    }
}

impl SolutionListener<SPProblem> for SvgExporter{
    fn report(&mut self, report_type: ReportType, solution: &SPSolution, instance: &SPInstance) {
        let suffix = match report_type {
            ReportType::CmprFeas => "cmpr",
            ReportType::ExplInfeas => "expl_nf",
            ReportType::ExplFeas => "expl_f",
            ReportType::Final => "final",
            ReportType::ExplImproving => "expl_i"
        };
        let file_name = format!("{}_{:.3}_{}", self.svg_counter, solution.strip_width(), suffix);
        if let Some(live_path) = &self.live_path {
            let svg = s_layout_to_svg(&solution.layout_snapshot, instance, DRAW_OPTIONS, file_name.as_str());
            io::write_svg(&svg, Path::new(live_path), Level::Trace).expect("failed to write live svg");
        }
        if let Some(intermediate_dir) = &self.intermediate_dir && report_type != ReportType::ExplImproving {
            let svg = s_layout_to_svg(&solution.layout_snapshot, instance, DRAW_OPTIONS, file_name.as_str());
            let file_path = &*format!("{intermediate_dir}/{file_name}.svg");
            io::write_svg(&svg, Path::new(file_path), Level::Trace).expect("failed to write intermediate svg");
            self.svg_counter += 1;
        }
        if let Some(final_path) = &self.final_path && report_type == ReportType::Final {
            let stem = Path::new(final_path).file_stem().unwrap();
            let svg = s_layout_to_svg(&solution.layout_snapshot, instance, DRAW_OPTIONS, stem.to_str().unwrap());
            io::write_svg(&svg, Path::new(final_path), Level::Info).expect("failed to write final svg");
        }
    }
}

/// BPP listener: emits one SVG per layout (bin). Files are suffixed with the
/// bin index, e.g. `final_swim_bin_0.svg`, `final_swim_bin_1.svg`, ...
impl SolutionListener<BPProblem> for SvgExporter {
    fn report(&mut self, report_type: ReportType, solution: &BPSolution, instance: &BPInstance) {
        let suffix = match report_type {
            ReportType::CmprFeas => "cmpr",
            ReportType::ExplInfeas => "expl_nf",
            ReportType::ExplFeas => "expl_f",
            ReportType::Final => "final",
            ReportType::ExplImproving => "expl_i",
        };
        let n_bins = solution.layout_snapshots.len();
        let base_name = format!("{}_{}b_{}", self.svg_counter, n_bins, suffix);

        // Live SVG: emit only the first layout (live viewer is single-document).
        if let Some(live_path) = &self.live_path {
            if let Some((_, snap)) = solution.layout_snapshots.iter().next() {
                let svg = s_layout_to_svg(snap, instance, DRAW_OPTIONS, base_name.as_str());
                io::write_svg(&svg, Path::new(live_path), Level::Trace)
                    .expect("failed to write live svg");
            }
        }

        // Intermediate SVGs: one file per bin, only on non-improving reports.
        if let Some(intermediate_dir) = &self.intermediate_dir
            && report_type != ReportType::ExplImproving
        {
            for (bin_idx, (_, snap)) in solution.layout_snapshots.iter().enumerate() {
                let file_name = format!("{base_name}_bin_{bin_idx}");
                let svg = s_layout_to_svg(snap, instance, DRAW_OPTIONS, file_name.as_str());
                let file_path = format!("{intermediate_dir}/{file_name}.svg");
                io::write_svg(&svg, Path::new(&file_path), Level::Trace)
                    .expect("failed to write intermediate svg");
            }
            self.svg_counter += 1;
        }

        // Final SVGs: one file per bin, plus an index summary in the log.
        if let Some(final_path) = &self.final_path
            && report_type == ReportType::Final
        {
            let final_path = Path::new(final_path);
            let stem = final_path.file_stem().unwrap().to_string_lossy().to_string();
            let parent = final_path.parent().unwrap_or(Path::new("."));
            for (bin_idx, (_, snap)) in solution.layout_snapshots.iter().enumerate() {
                let file_name = format!("{stem}_bin_{bin_idx}");
                let svg = s_layout_to_svg(snap, instance, DRAW_OPTIONS, file_name.as_str());
                let file_path = parent.join(format!("{file_name}.svg"));
                io::write_svg(&svg, &file_path, Level::Info)
                    .expect("failed to write final svg");
            }
        }
    }
}