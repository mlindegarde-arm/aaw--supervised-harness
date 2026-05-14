use crate::domain::{Run, TaskId, Ticket, TicketId, TicketResolution, TicketStatus};
use crate::service::SupervisorStateView;
use crate::{HarnessError, HarnessResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorStuckRun {
    pub run: Run,
    pub next_cycle: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorTicketSelection {
    ResolvedUnconsumed {
        ticket: Ticket,
        resolution: TicketResolution,
    },
    Open {
        ticket: Ticket,
    },
    RetryableResolving {
        ticket: Ticket,
    },
    RetryableFailed {
        ticket: Ticket,
    },
}

impl SupervisorTicketSelection {
    pub fn ticket(&self) -> &Ticket {
        match self {
            Self::ResolvedUnconsumed { ticket, .. }
            | Self::Open { ticket }
            | Self::RetryableResolving { ticket }
            | Self::RetryableFailed { ticket } => ticket,
        }
    }

    pub fn resolution(&self) -> Option<&TicketResolution> {
        match self {
            Self::ResolvedUnconsumed { resolution, .. } => Some(resolution),
            Self::Open { .. } | Self::RetryableResolving { .. } | Self::RetryableFailed { .. } => {
                None
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorStuckSelection {
    pub stuck_run: SupervisorStuckRun,
    pub ticket: SupervisorTicketSelection,
}

pub fn latest_stuck_run<V>(state: &V, task_id: &TaskId) -> HarnessResult<Option<SupervisorStuckRun>>
where
    V: SupervisorStateView + ?Sized,
{
    Ok(state
        .latest_stuck_run(task_id)?
        .map(|run| SupervisorStuckRun {
            next_cycle: next_resume_cycle_for_run(&run),
            run,
        }))
}

pub fn next_resume_cycle_for_run(run: &Run) -> u32 {
    run.escalation_cycle.saturating_add(1)
}

pub fn next_resume_cycle<V>(state: &V, task_id: &TaskId) -> HarnessResult<Option<u32>>
where
    V: SupervisorStateView + ?Sized,
{
    Ok(latest_stuck_run(state, task_id)?.map(|stuck| stuck.next_cycle))
}

pub fn select_ticket_for_latest_stuck_run<V>(
    state: &V,
    task_id: &TaskId,
    requested_ticket: Option<&TicketId>,
) -> HarnessResult<Option<SupervisorStuckSelection>>
where
    V: SupervisorStateView + ?Sized,
{
    let Some(stuck_run) = latest_stuck_run(state, task_id)? else {
        return Ok(None);
    };

    let mut tickets = state.tickets_for_run(&stuck_run.run.id)?;
    tickets.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });

    let ticket = match requested_ticket {
        Some(ticket_id) => {
            let ticket = tickets
                .into_iter()
                .find(|ticket| &ticket.id == ticket_id)
                .ok_or_else(|| stale_ticket_error(task_id, ticket_id, &stuck_run.run))?;
            select_specific_ticket(state, task_id, &stuck_run.run, ticket)?
        }
        None => select_preferred_ticket(state, task_id, &stuck_run.run, &tickets)?,
    };

    Ok(ticket.map(|ticket| SupervisorStuckSelection { stuck_run, ticket }))
}

pub fn reject_stale_ticket(
    task_id: &TaskId,
    latest_stuck_run: &Run,
    ticket: &Ticket,
) -> HarnessResult<()> {
    if &ticket.task_id != task_id || ticket.run_id != latest_stuck_run.id {
        return Err(stale_ticket_error(task_id, &ticket.id, latest_stuck_run));
    }
    Ok(())
}

fn select_preferred_ticket<V>(
    state: &V,
    task_id: &TaskId,
    latest_stuck_run: &Run,
    tickets: &[Ticket],
) -> HarnessResult<Option<SupervisorTicketSelection>>
where
    V: SupervisorStateView + ?Sized,
{
    for ticket in tickets {
        if ticket.status == TicketStatus::Resolved {
            reject_stale_ticket(task_id, latest_stuck_run, ticket)?;
            if let Some(resolution) = state.latest_unconsumed_resolution_for_ticket(&ticket.id)? {
                return Ok(Some(SupervisorTicketSelection::ResolvedUnconsumed {
                    ticket: ticket.clone(),
                    resolution,
                }));
            }
        }
    }

    if let Some(ticket) = tickets
        .iter()
        .find(|ticket| ticket.status == TicketStatus::Open)
    {
        reject_stale_ticket(task_id, latest_stuck_run, ticket)?;
        return Ok(Some(SupervisorTicketSelection::Open {
            ticket: ticket.clone(),
        }));
    }

    if let Some(ticket) = tickets
        .iter()
        .find(|ticket| ticket.status == TicketStatus::Resolving)
    {
        reject_stale_ticket(task_id, latest_stuck_run, ticket)?;
        return Ok(Some(SupervisorTicketSelection::RetryableResolving {
            ticket: ticket.clone(),
        }));
    }

    if let Some(ticket) = tickets
        .iter()
        .find(|ticket| ticket.status == TicketStatus::Failed)
    {
        reject_stale_ticket(task_id, latest_stuck_run, ticket)?;
        return Ok(Some(SupervisorTicketSelection::RetryableFailed {
            ticket: ticket.clone(),
        }));
    }

    Ok(None)
}

fn select_specific_ticket<V>(
    state: &V,
    task_id: &TaskId,
    latest_stuck_run: &Run,
    ticket: Ticket,
) -> HarnessResult<Option<SupervisorTicketSelection>>
where
    V: SupervisorStateView + ?Sized,
{
    reject_stale_ticket(task_id, latest_stuck_run, &ticket)?;
    match ticket.status {
        TicketStatus::Resolved => {
            let resolution = state
                .latest_unconsumed_resolution_for_ticket(&ticket.id)?
                .ok_or_else(|| {
                    HarnessError::Conflict(format!(
                        "ticket {} has no unconsumed resolution",
                        ticket.id
                    ))
                })?;
            Ok(Some(SupervisorTicketSelection::ResolvedUnconsumed {
                ticket,
                resolution,
            }))
        }
        TicketStatus::Open => Ok(Some(SupervisorTicketSelection::Open { ticket })),
        TicketStatus::Resolving => Ok(Some(SupervisorTicketSelection::RetryableResolving {
            ticket,
        })),
        TicketStatus::Failed => Ok(Some(SupervisorTicketSelection::RetryableFailed { ticket })),
    }
}

fn stale_ticket_error(
    task_id: &TaskId,
    ticket_id: &TicketId,
    latest_stuck_run: &Run,
) -> HarnessError {
    HarnessError::Conflict(format!(
        "ticket {} is stale for task {}; expected a ticket on latest stuck run {}",
        ticket_id, task_id, latest_stuck_run.id
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{RunId, RunStatus, Task, TaskStatus};
    use crate::service::SupervisorTaskState;
    use std::collections::BTreeMap;

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const OTHER_TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const RUN_1: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_2: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const TICKET_1: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_2: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const TICKET_3: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAX";
    const TICKET_4: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAY";

    #[test]
    fn latest_stuck_run_helper_returns_persisted_next_cycle() {
        let task_id = task_id();
        let state = FakeSupervisorState::new(task_id.clone()).with_run(run(
            RUN_1,
            &task_id,
            RunStatus::Stuck,
            "2026-01-01T00:00:01Z",
            2,
        ));

        let stuck = latest_stuck_run(&state, &task_id).unwrap().unwrap();

        assert_eq!(stuck.run.id.as_str(), RUN_1);
        assert_eq!(stuck.next_cycle, 3);
        assert_eq!(next_resume_cycle(&state, &task_id).unwrap(), Some(3));
    }

    #[test]
    fn task_scoped_ticket_selection_uses_latest_stuck_run_only() {
        let task_id = task_id();
        let state = FakeSupervisorState::new(task_id.clone())
            .with_run(run(
                RUN_1,
                &task_id,
                RunStatus::Stuck,
                "2026-01-01T00:00:01Z",
                0,
            ))
            .with_run(run(
                RUN_2,
                &task_id,
                RunStatus::Stuck,
                "2026-01-01T00:00:02Z",
                1,
            ))
            .with_ticket(ticket(
                TICKET_1,
                &task_id,
                RUN_1,
                TicketStatus::Open,
                "2026-01-01T00:00:03Z",
            ))
            .with_ticket(ticket(
                TICKET_2,
                &task_id,
                RUN_2,
                TicketStatus::Open,
                "2026-01-01T00:00:04Z",
            ));

        let selected = select_ticket_for_latest_stuck_run(&state, &task_id, None)
            .unwrap()
            .unwrap();

        assert_eq!(selected.stuck_run.run.id.as_str(), RUN_2);
        assert_eq!(selected.ticket.ticket().id.as_str(), TICKET_2);
    }

    #[test]
    fn stale_requested_ticket_is_rejected() {
        let task_id = task_id();
        let state = FakeSupervisorState::new(task_id.clone())
            .with_run(run(
                RUN_1,
                &task_id,
                RunStatus::Stuck,
                "2026-01-01T00:00:01Z",
                0,
            ))
            .with_run(run(
                RUN_2,
                &task_id,
                RunStatus::Stuck,
                "2026-01-01T00:00:02Z",
                1,
            ))
            .with_ticket(ticket(
                TICKET_1,
                &task_id,
                RUN_1,
                TicketStatus::Open,
                "2026-01-01T00:00:03Z",
            ))
            .with_ticket(ticket(
                TICKET_2,
                &task_id,
                RUN_2,
                TicketStatus::Open,
                "2026-01-01T00:00:04Z",
            ));

        let error = select_ticket_for_latest_stuck_run(
            &state,
            &task_id,
            Some(&TicketId::parse(TICKET_1).unwrap()),
        )
        .unwrap_err();

        assert!(error.to_string().contains("stale"));
        assert!(error.to_string().contains(RUN_2));
    }

    #[test]
    fn resolved_unconsumed_ticket_is_preferred_over_open_ticket() {
        let task_id = task_id();
        let resolved = ticket(
            TICKET_1,
            &task_id,
            RUN_1,
            TicketStatus::Resolved,
            "2026-01-01T00:00:03Z",
        );
        let open = ticket(
            TICKET_2,
            &task_id,
            RUN_1,
            TicketStatus::Open,
            "2026-01-01T00:00:04Z",
        );
        let state = FakeSupervisorState::new(task_id.clone())
            .with_run(run(
                RUN_1,
                &task_id,
                RunStatus::Stuck,
                "2026-01-01T00:00:01Z",
                0,
            ))
            .with_ticket(open)
            .with_ticket(resolved.clone())
            .with_resolution(resolution(
                "res_01ARZ3NDEKTSV4RRFFQ69G5FAV",
                &resolved.id,
                None,
            ));

        let selected = select_ticket_for_latest_stuck_run(&state, &task_id, None)
            .unwrap()
            .unwrap();

        assert!(matches!(
            selected.ticket,
            SupervisorTicketSelection::ResolvedUnconsumed { .. }
        ));
        assert_eq!(selected.ticket.ticket().id.as_str(), TICKET_1);
        assert!(selected.ticket.resolution().is_some());
    }

    #[test]
    fn consumed_resolved_ticket_falls_back_to_open_ticket() {
        let task_id = task_id();
        let resolved = ticket(
            TICKET_1,
            &task_id,
            RUN_1,
            TicketStatus::Resolved,
            "2026-01-01T00:00:04Z",
        );
        let open = ticket(
            TICKET_2,
            &task_id,
            RUN_1,
            TicketStatus::Open,
            "2026-01-01T00:00:03Z",
        );
        let state = FakeSupervisorState::new(task_id.clone())
            .with_run(run(
                RUN_1,
                &task_id,
                RunStatus::Stuck,
                "2026-01-01T00:00:01Z",
                0,
            ))
            .with_ticket(resolved.clone())
            .with_ticket(open)
            .with_resolution(resolution(
                "res_01ARZ3NDEKTSV4RRFFQ69G5FAV",
                &resolved.id,
                Some("2026-01-01T00:00:05Z"),
            ));

        let selected = select_ticket_for_latest_stuck_run(&state, &task_id, None)
            .unwrap()
            .unwrap();

        assert!(matches!(
            selected.ticket,
            SupervisorTicketSelection::Open { .. }
        ));
        assert_eq!(selected.ticket.ticket().id.as_str(), TICKET_2);
    }

    #[test]
    fn retryable_resolving_is_preferred_before_retryable_failed() {
        let task_id = task_id();
        let state = FakeSupervisorState::new(task_id.clone())
            .with_run(run(
                RUN_1,
                &task_id,
                RunStatus::Stuck,
                "2026-01-01T00:00:01Z",
                0,
            ))
            .with_ticket(ticket(
                TICKET_3,
                &task_id,
                RUN_1,
                TicketStatus::Failed,
                "2026-01-01T00:00:04Z",
            ))
            .with_ticket(ticket(
                TICKET_4,
                &task_id,
                RUN_1,
                TicketStatus::Resolving,
                "2026-01-01T00:00:03Z",
            ));

        let selected = select_ticket_for_latest_stuck_run(&state, &task_id, None)
            .unwrap()
            .unwrap();

        assert!(matches!(
            selected.ticket,
            SupervisorTicketSelection::RetryableResolving { .. }
        ));
        assert_eq!(selected.ticket.ticket().id.as_str(), TICKET_4);
    }

    #[test]
    fn retryable_failed_is_selected_when_no_better_ticket_exists() {
        let task_id = task_id();
        let state = FakeSupervisorState::new(task_id.clone())
            .with_run(run(
                RUN_1,
                &task_id,
                RunStatus::Stuck,
                "2026-01-01T00:00:01Z",
                0,
            ))
            .with_ticket(ticket(
                TICKET_3,
                &task_id,
                RUN_1,
                TicketStatus::Failed,
                "2026-01-01T00:00:04Z",
            ));

        let selected = select_ticket_for_latest_stuck_run(&state, &task_id, None)
            .unwrap()
            .unwrap();

        assert!(matches!(
            selected.ticket,
            SupervisorTicketSelection::RetryableFailed { .. }
        ));
        assert_eq!(selected.ticket.ticket().id.as_str(), TICKET_3);
    }

    #[test]
    fn no_selectable_ticket_returns_none_without_mutating_state() {
        let task_id = task_id();
        let state = FakeSupervisorState::new(task_id.clone()).with_run(run(
            RUN_1,
            &task_id,
            RunStatus::Stuck,
            "2026-01-01T00:00:01Z",
            0,
        ));

        let selected = select_ticket_for_latest_stuck_run(&state, &task_id, None).unwrap();

        assert!(selected.is_none());
    }

    fn task_id() -> TaskId {
        TaskId::parse(TASK_ID).unwrap()
    }

    #[derive(Debug, Clone)]
    struct FakeSupervisorState {
        task_id: TaskId,
        runs: Vec<Run>,
        tickets: Vec<Ticket>,
        resolutions: BTreeMap<TicketId, Vec<TicketResolution>>,
    }

    impl FakeSupervisorState {
        fn new(task_id: TaskId) -> Self {
            Self {
                task_id,
                runs: Vec::new(),
                tickets: Vec::new(),
                resolutions: BTreeMap::new(),
            }
        }

        fn with_run(mut self, run: Run) -> Self {
            self.runs.push(run);
            self
        }

        fn with_ticket(mut self, ticket: Ticket) -> Self {
            self.tickets.push(ticket);
            self
        }

        fn with_resolution(mut self, resolution: TicketResolution) -> Self {
            self.resolutions
                .entry(resolution.ticket_id.clone())
                .or_default()
                .push(resolution);
            self
        }
    }

    impl SupervisorStateView for FakeSupervisorState {
        fn supervisor_task_state(&self, task_id: &TaskId) -> HarnessResult<SupervisorTaskState> {
            Ok(SupervisorTaskState {
                task: task(task_id.clone()),
                latest_run: self.latest_stuck_run(task_id)?,
                tickets: self
                    .tickets
                    .iter()
                    .filter(|ticket| &ticket.task_id == task_id)
                    .cloned()
                    .collect(),
                unconsumed_resolution: None,
            })
        }

        fn latest_stuck_run(&self, task_id: &TaskId) -> HarnessResult<Option<Run>> {
            let mut runs = self
                .runs
                .iter()
                .filter(|run| &run.task_id == task_id && run.status == RunStatus::Stuck)
                .cloned()
                .collect::<Vec<_>>();
            runs.sort_by(|left, right| {
                right
                    .started_at
                    .cmp(&left.started_at)
                    .then_with(|| right.id.cmp(&left.id))
            });
            Ok(runs.into_iter().next())
        }

        fn tickets_for_run(&self, run_id: &RunId) -> HarnessResult<Vec<Ticket>> {
            Ok(self
                .tickets
                .iter()
                .filter(|ticket| &ticket.run_id == run_id && ticket.task_id == self.task_id)
                .cloned()
                .collect())
        }

        fn latest_unconsumed_resolution_for_ticket(
            &self,
            ticket_id: &TicketId,
        ) -> HarnessResult<Option<TicketResolution>> {
            let mut resolutions = self
                .resolutions
                .get(ticket_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|resolution| resolution.consumed_at.is_none())
                .collect::<Vec<_>>();
            resolutions.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| right.id.cmp(&left.id))
            });
            Ok(resolutions.into_iter().next())
        }
    }

    fn task(id: TaskId) -> Task {
        Task {
            id,
            title: "Fix tests".to_string(),
            goal: "Make tests pass".to_string(),
            status: TaskStatus::Stuck,
            repo_root: "/repo".to_string(),
            worktree_path: None,
            branch: None,
            base_ref: Some("main".to_string()),
            base_commit: Some("abcdef".to_string()),
            last_seen_head: None,
            max_attempts: 3,
            lease_owner: None,
            lease_acquired_at: None,
            lease_expires_at: None,
            heartbeat_at: None,
            lock_version: 0,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn run(
        id: &str,
        task_id: &TaskId,
        status: RunStatus,
        started_at: &str,
        escalation_cycle: u32,
    ) -> Run {
        Run {
            id: RunId::parse(id).unwrap(),
            task_id: task_id.clone(),
            parent_run_id: None,
            status,
            repo_root: "/repo".to_string(),
            base_ref: Some("main".to_string()),
            base_commit: "abcdef".to_string(),
            dirty_state_summary: None,
            current_phase: Some("stuck".to_string()),
            escalation_cycle,
            started_at: started_at.to_string(),
            finished_at: Some(started_at.to_string()),
            final_diff_path: None,
            last_error: None,
        }
    }

    fn ticket(
        id: &str,
        task_id: &TaskId,
        run_id: &str,
        status: TicketStatus,
        created_at: &str,
    ) -> Ticket {
        Ticket {
            id: TicketId::parse(id).unwrap(),
            task_id: task_id.clone(),
            run_id: RunId::parse(run_id).unwrap(),
            status,
            blocked_on: "validation".to_string(),
            question: "What next?".to_string(),
            reason: "stuck".to_string(),
            evidence_json: "{}".to_string(),
            failure_fingerprint: id.to_string(),
            created_at: created_at.to_string(),
            resolved_at: None,
        }
    }

    fn resolution(id: &str, ticket_id: &TicketId, consumed_at: Option<&str>) -> TicketResolution {
        TicketResolution {
            id: crate::domain::TicketResolutionId::parse(id).unwrap(),
            ticket_id: ticket_id.clone(),
            provider: "fake-openai".to_string(),
            model: "fake".to_string(),
            response_id: None,
            resolution_path: "resolution.md".to_string(),
            consumed_at: consumed_at.map(str::to_string),
            created_at: "2026-01-01T00:00:05Z".to_string(),
        }
    }

    #[test]
    fn reject_stale_ticket_rejects_other_task_even_with_same_run_id() {
        let task_id = task_id();
        let latest = run(RUN_1, &task_id, RunStatus::Stuck, "2026-01-01T00:00:01Z", 0);
        let mut stale = ticket(
            TICKET_1,
            &TaskId::parse(OTHER_TASK_ID).unwrap(),
            RUN_1,
            TicketStatus::Open,
            "2026-01-01T00:00:03Z",
        );
        stale.run_id = latest.id.clone();

        let error = reject_stale_ticket(&task_id, &latest, &stale).unwrap_err();

        assert!(error.to_string().contains("stale"));
    }
}
