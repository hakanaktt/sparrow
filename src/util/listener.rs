use crate::optimizer::problem::PackingProblem;

/// Trait for listeners that can receive solutions during the optimization process.
///
/// Generic over the packing-problem type `P` so that both SPP and BPP solutions
/// (and any future variant) can share the same listener mechanism.
pub trait SolutionListener<P: PackingProblem> {
    fn report(&mut self, report: ReportType, solution: &P::Solution, instance: &P::Instance);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportType {
    /// Report contains a feasible solution reached by the exploration phase.
    ExplFeas,
    /// Report contains an infeasible solution reached by the exploration phase.
    ExplInfeas,
    /// Report contains an intermediate solution from the exploration phase that is closer to feasibility than the previous one.
    ExplImproving,
    /// Report contains a feasible solution from the comparison phase.
    CmprFeas,
    /// Report contains the final solution
    Final,
}

/// A dummy implementation of the `SolutionListener` trait that does nothing.
///
/// Generic so it works for any problem type without per-problem dummy structs.
pub struct DummySolListener;

impl<P: PackingProblem> SolutionListener<P> for DummySolListener {
    fn report(&mut self, _report: ReportType, _solution: &P::Solution, _instance: &P::Instance) {
        // Do nothing
    }
}
