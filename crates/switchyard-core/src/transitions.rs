use crate::domain::{CommandKind, Phase};
use crate::error::{Error, Result};

/// Where an allowed edge lands. Dynamic targets are resolved by policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetSpec {
    Fixed(Phase),
    SavedResume,
    ValidatedRecovery,
}

/// Table lookup for one phase and one transition command.
///
/// `Steer` is not a transition. Terminal phases return
/// [`Error::TerminalImmutable`]. Every other unlisted pair is
/// [`Error::IllegalTransition`].
pub fn target_spec(from: Phase, command: CommandKind) -> Result<TargetSpec> {
    if from.is_terminal() {
        return Err(Error::TerminalImmutable {
            phase: from.to_string(),
        });
    }
    if command == CommandKind::Steer {
        return Err(Error::IllegalTransition {
            from: from.to_string(),
            command: command.to_string(),
        });
    }
    let spec = match (from, command) {
        (Phase::Draft, CommandKind::StartPlanning) => TargetSpec::Fixed(Phase::Planning),
        (Phase::Planning, CommandKind::PlanCompleted) => {
            TargetSpec::Fixed(Phase::AwaitingPlanApproval)
        }
        (Phase::AwaitingPlanApproval, CommandKind::RequestRevisedPlan) => {
            TargetSpec::Fixed(Phase::Planning)
        }
        (Phase::AwaitingPlanApproval, CommandKind::ApprovePlan) => {
            TargetSpec::Fixed(Phase::Implementing)
        }
        (Phase::Implementing, CommandKind::RequestVerification) => {
            TargetSpec::Fixed(Phase::Verifying)
        }
        (Phase::Verifying, CommandKind::CompletePass) => TargetSpec::Fixed(Phase::AwaitingReview),
        (Phase::Verifying, CommandKind::AuthorizeBoundedRepair) => {
            TargetSpec::Fixed(Phase::Implementing)
        }
        (Phase::AwaitingReview, CommandKind::RequestChanges) => {
            TargetSpec::Fixed(Phase::Implementing)
        }
        (Phase::AwaitingReview, CommandKind::ApproveReview) => TargetSpec::Fixed(Phase::Approved),
        (Phase::Approved, CommandKind::Publish) => TargetSpec::Fixed(Phase::Publishing),
        (Phase::Publishing, CommandKind::ReconcileDeliveryComplete) => {
            TargetSpec::Fixed(Phase::Published)
        }
        (
            Phase::AwaitingPlanApproval | Phase::AwaitingReview | Phase::Approved,
            CommandKind::Reject,
        ) => TargetSpec::Fixed(Phase::Rejected),
        (from, CommandKind::Pause) if from.is_pause_source() => TargetSpec::Fixed(Phase::Pausing),
        (Phase::Pausing, CommandKind::ConfirmStopped) => TargetSpec::Fixed(Phase::Paused),
        (Phase::Paused, CommandKind::Resume) => TargetSpec::SavedResume,
        (from, CommandKind::Cancel) if from != Phase::Cancelling => {
            TargetSpec::Fixed(Phase::Cancelling)
        }
        (Phase::Cancelling, CommandKind::ConfirmCancelled) => TargetSpec::Fixed(Phase::Cancelled),
        (Phase::Cancelling, CommandKind::DiscoverPublicationCompleted) => {
            TargetSpec::Fixed(Phase::Published)
        }
        (from, CommandKind::DetectUncertainty) if from != Phase::NeedsAttention => {
            TargetSpec::Fixed(Phase::NeedsAttention)
        }
        (Phase::NeedsAttention, CommandKind::ExplicitRecovery) => TargetSpec::ValidatedRecovery,
        _ => {
            return Err(Error::IllegalTransition {
                from: from.to_string(),
                command: command.to_string(),
            });
        }
    };
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_edges() -> Vec<(Phase, CommandKind, TargetSpec)> {
        let mut edges = vec![
            (
                Phase::Draft,
                CommandKind::StartPlanning,
                TargetSpec::Fixed(Phase::Planning),
            ),
            (
                Phase::Planning,
                CommandKind::PlanCompleted,
                TargetSpec::Fixed(Phase::AwaitingPlanApproval),
            ),
            (
                Phase::AwaitingPlanApproval,
                CommandKind::RequestRevisedPlan,
                TargetSpec::Fixed(Phase::Planning),
            ),
            (
                Phase::AwaitingPlanApproval,
                CommandKind::ApprovePlan,
                TargetSpec::Fixed(Phase::Implementing),
            ),
            (
                Phase::Implementing,
                CommandKind::RequestVerification,
                TargetSpec::Fixed(Phase::Verifying),
            ),
            (
                Phase::Verifying,
                CommandKind::CompletePass,
                TargetSpec::Fixed(Phase::AwaitingReview),
            ),
            (
                Phase::Verifying,
                CommandKind::AuthorizeBoundedRepair,
                TargetSpec::Fixed(Phase::Implementing),
            ),
            (
                Phase::AwaitingReview,
                CommandKind::RequestChanges,
                TargetSpec::Fixed(Phase::Implementing),
            ),
            (
                Phase::AwaitingReview,
                CommandKind::ApproveReview,
                TargetSpec::Fixed(Phase::Approved),
            ),
            (
                Phase::Approved,
                CommandKind::Publish,
                TargetSpec::Fixed(Phase::Publishing),
            ),
            (
                Phase::Publishing,
                CommandKind::ReconcileDeliveryComplete,
                TargetSpec::Fixed(Phase::Published),
            ),
            (
                Phase::Pausing,
                CommandKind::ConfirmStopped,
                TargetSpec::Fixed(Phase::Paused),
            ),
            (Phase::Paused, CommandKind::Resume, TargetSpec::SavedResume),
            (
                Phase::Cancelling,
                CommandKind::ConfirmCancelled,
                TargetSpec::Fixed(Phase::Cancelled),
            ),
            (
                Phase::Cancelling,
                CommandKind::DiscoverPublicationCompleted,
                TargetSpec::Fixed(Phase::Published),
            ),
            (
                Phase::NeedsAttention,
                CommandKind::ExplicitRecovery,
                TargetSpec::ValidatedRecovery,
            ),
        ];
        for from in [
            Phase::AwaitingPlanApproval,
            Phase::AwaitingReview,
            Phase::Approved,
        ] {
            edges.push((
                from,
                CommandKind::Reject,
                TargetSpec::Fixed(Phase::Rejected),
            ));
        }
        for from in Phase::all()
            .into_iter()
            .filter(|phase| phase.is_pause_source())
        {
            edges.push((from, CommandKind::Pause, TargetSpec::Fixed(Phase::Pausing)));
        }
        for from in Phase::all()
            .into_iter()
            .filter(|phase| !phase.is_terminal() && *phase != Phase::Cancelling)
        {
            edges.push((
                from,
                CommandKind::Cancel,
                TargetSpec::Fixed(Phase::Cancelling),
            ));
        }
        for from in Phase::all()
            .into_iter()
            .filter(|phase| !phase.is_terminal() && *phase != Phase::NeedsAttention)
        {
            edges.push((
                from,
                CommandKind::DetectUncertainty,
                TargetSpec::Fixed(Phase::NeedsAttention),
            ));
        }
        edges
    }

    #[test]
    fn generated_matrix_matches_the_normative_table() {
        let expected = spec_edges();
        assert_eq!(
            expected.len(),
            48,
            "the normative table has 48 concrete edges"
        );
        let mut seen = std::collections::HashSet::new();
        for (from, command, spec) in &expected {
            assert!(
                seen.insert((*from, *command)),
                "duplicate edge {from} via {command}"
            );
            assert_eq!(target_spec(*from, *command).unwrap(), *spec);
        }

        let mut allowed = 0;
        let mut rejected = 0;
        for from in Phase::all() {
            for command in CommandKind::transition_kinds() {
                let result = target_spec(from, command);
                let listed = seen.contains(&(from, command));
                if listed {
                    assert!(result.is_ok(), "{from} via {command} should be allowed");
                    allowed += 1;
                } else if from.is_terminal() {
                    assert!(
                        matches!(result, Err(Error::TerminalImmutable { .. })),
                        "{from} via {command} should be terminal_immutable, got {result:?}"
                    );
                    rejected += 1;
                } else {
                    assert!(
                        matches!(result, Err(Error::IllegalTransition { .. })),
                        "{from} via {command} should be illegal, got {result:?}"
                    );
                    rejected += 1;
                }
            }
        }
        assert_eq!(allowed, 48);
        assert_eq!(
            allowed + rejected,
            Phase::all().len() * CommandKind::transition_kinds().len()
        );
        assert!(
            !Phase::Publishing.is_pause_source(),
            "pause during publishing is not an allowed edge"
        );
    }
}
