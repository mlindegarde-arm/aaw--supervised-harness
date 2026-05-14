use crate::HarnessResult;
use crate::domain::{Run, RunId, Task, TaskId, Ticket, TicketId, TicketResolution};
use crate::runtime::{
    CommandEvent, CommandExit, CommandResult, CommandStatus, SuperviseCreateOptions,
    SuperviseTaskOptions,
};
use serde_json::json;

pub const SUPERVISOR_PLACEHOLDER_MESSAGE: &str =
    "supervise is not implemented yet; supervisor loop belongs to Phase 2 supervisor workstream";

pub fn supervise_placeholder_result(task_id: Option<&TaskId>) -> CommandResult {
    let mut data = json!({
        "implemented": false,
        "next": "harness task run <task-id>",
    });
    if let Some(task_id) = task_id {
        data["task_id"] = json!(task_id.as_str());
        data["next"] = json!(format!(
            "harness supervise {} --output json",
            task_id.as_str()
        ));
    }

    CommandResult::with_data(
        CommandExit::new(
            CommandStatus::Failed,
            1,
            Some(SUPERVISOR_PLACEHOLDER_MESSAGE.to_string()),
        ),
        data,
    )
    .with_event(CommandEvent::warn(
        "supervisor.placeholder",
        SUPERVISOR_PLACEHOLDER_MESSAGE,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorTaskState {
    pub task: Task,
    pub latest_run: Option<Run>,
    pub tickets: Vec<Ticket>,
    pub unconsumed_resolution: Option<TicketResolution>,
}

pub trait SupervisorStateView {
    fn supervisor_task_state(&self, task_id: &TaskId) -> HarnessResult<SupervisorTaskState>;
    fn latest_stuck_run(&self, task_id: &TaskId) -> HarnessResult<Option<Run>>;
    fn tickets_for_run(&self, run_id: &RunId) -> HarnessResult<Vec<Ticket>>;
    fn latest_unconsumed_resolution_for_ticket(
        &self,
        ticket_id: &TicketId,
    ) -> HarnessResult<Option<TicketResolution>>;
}

pub trait SupervisorStateStore: SupervisorStateView {
    fn recover_expired_supervisor_leases(&self, task_id: &TaskId) -> HarnessResult<()>;
    fn mark_supervisor_cycle_started(&self, task_id: &TaskId, cycle: u32) -> HarnessResult<()>;
}

pub trait SupervisorService {
    fn supervise_task(
        &self,
        task_id: &TaskId,
        options: SuperviseTaskOptions,
    ) -> HarnessResult<CommandResult>;

    fn create_and_supervise_task(
        &self,
        options: SuperviseCreateOptions,
    ) -> HarnessResult<CommandResult>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{RunStatus, TaskStatus, TicketStatus};
    use crate::runtime::RuntimeOptions;

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_ID: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_ID: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn service_supervisor_placeholder_is_deterministic_failure() {
        let task_id = TaskId::parse(TASK_ID).unwrap();

        let result = supervise_placeholder_result(Some(&task_id));

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(result.exit.exit_code, 1);
        assert_eq!(
            result.exit.message.as_deref(),
            Some(SUPERVISOR_PLACEHOLDER_MESSAGE)
        );
        assert_eq!(result.data["implemented"], false);
        assert_eq!(result.data["task_id"], TASK_ID);
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].kind, "supervisor.placeholder");
    }

    #[test]
    fn service_supervisor_trait_surface_compiles_for_later_implementations() {
        struct StubSupervisor;

        impl SupervisorService for StubSupervisor {
            fn supervise_task(
                &self,
                task_id: &TaskId,
                _options: SuperviseTaskOptions,
            ) -> HarnessResult<CommandResult> {
                Ok(supervise_placeholder_result(Some(task_id)))
            }

            fn create_and_supervise_task(
                &self,
                _options: SuperviseCreateOptions,
            ) -> HarnessResult<CommandResult> {
                Ok(supervise_placeholder_result(None))
            }
        }

        let task_id = TaskId::parse(TASK_ID).unwrap();
        let service = StubSupervisor;
        let result = service
            .supervise_task(&task_id, SuperviseTaskOptions::default())
            .unwrap();
        let created = service
            .create_and_supervise_task(SuperviseCreateOptions::new(
                "Fix",
                "Goal",
                vec!["cargo test".to_string()],
            ))
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(created.exit.status, CommandStatus::Failed);
    }

    #[test]
    fn service_supervisor_state_traits_compile_without_sqlite_behavior() {
        struct StubState;

        impl SupervisorStateView for StubState {
            fn supervisor_task_state(
                &self,
                task_id: &TaskId,
            ) -> HarnessResult<SupervisorTaskState> {
                Ok(SupervisorTaskState {
                    task: task(task_id.clone()),
                    latest_run: Some(run(task_id.clone())),
                    tickets: vec![ticket(task_id.clone())],
                    unconsumed_resolution: None,
                })
            }

            fn latest_stuck_run(&self, task_id: &TaskId) -> HarnessResult<Option<Run>> {
                Ok(Some(run(task_id.clone())))
            }

            fn tickets_for_run(&self, _run_id: &RunId) -> HarnessResult<Vec<Ticket>> {
                Ok(vec![ticket(TaskId::parse(TASK_ID).unwrap())])
            }

            fn latest_unconsumed_resolution_for_ticket(
                &self,
                _ticket_id: &TicketId,
            ) -> HarnessResult<Option<TicketResolution>> {
                Ok(None)
            }
        }

        impl SupervisorStateStore for StubState {
            fn recover_expired_supervisor_leases(&self, _task_id: &TaskId) -> HarnessResult<()> {
                Ok(())
            }

            fn mark_supervisor_cycle_started(
                &self,
                _task_id: &TaskId,
                _cycle: u32,
            ) -> HarnessResult<()> {
                Ok(())
            }
        }

        let state = StubState;
        let task_id = TaskId::parse(TASK_ID).unwrap();
        let snapshot = state.supervisor_task_state(&task_id).unwrap();

        assert_eq!(snapshot.task.status, TaskStatus::Ready);
        assert_eq!(snapshot.latest_run.unwrap().status, RunStatus::Stuck);
        assert_eq!(snapshot.tickets[0].status, TicketStatus::Open);
        state.recover_expired_supervisor_leases(&task_id).unwrap();
        state.mark_supervisor_cycle_started(&task_id, 1).unwrap();
    }

    fn task(id: TaskId) -> Task {
        Task {
            id,
            title: "title".to_string(),
            goal: "goal".to_string(),
            status: TaskStatus::Ready,
            repo_root: "/repo".to_string(),
            worktree_path: None,
            branch: None,
            base_ref: None,
            base_commit: None,
            last_seen_head: None,
            max_attempts: 3,
            lease_owner: None,
            lease_acquired_at: None,
            lease_expires_at: None,
            heartbeat_at: None,
            lock_version: 1,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        }
    }

    fn run(task_id: TaskId) -> Run {
        Run {
            id: RunId::parse(RUN_ID).unwrap(),
            task_id,
            parent_run_id: None,
            status: RunStatus::Stuck,
            repo_root: "/repo".to_string(),
            base_ref: None,
            base_commit: "abc".to_string(),
            dirty_state_summary: None,
            current_phase: Some("validation".to_string()),
            escalation_cycle: 1,
            started_at: "now".to_string(),
            finished_at: None,
            final_diff_path: None,
            last_error: Some("stuck".to_string()),
        }
    }

    fn ticket(task_id: TaskId) -> Ticket {
        Ticket {
            id: TicketId::parse(TICKET_ID).unwrap(),
            task_id,
            run_id: RunId::parse(RUN_ID).unwrap(),
            status: TicketStatus::Open,
            blocked_on: "validation".to_string(),
            question: "What next?".to_string(),
            reason: "stuck".to_string(),
            evidence_json: "{}".to_string(),
            failure_fingerprint: "abc".to_string(),
            created_at: "now".to_string(),
            resolved_at: None,
        }
    }

    #[test]
    fn service_supervisor_options_can_be_passed_through_trait_methods() {
        struct CapturingSupervisor;

        impl SupervisorService for CapturingSupervisor {
            fn supervise_task(
                &self,
                _task_id: &TaskId,
                options: SuperviseTaskOptions,
            ) -> HarnessResult<CommandResult> {
                assert_eq!(options.max_cycles, Some(2));
                assert_eq!(options.ticket_model.as_deref(), Some("ticket-model"));
                Ok(supervise_placeholder_result(None))
            }

            fn create_and_supervise_task(
                &self,
                options: SuperviseCreateOptions,
            ) -> HarnessResult<CommandResult> {
                assert_eq!(options.runtime, RuntimeOptions::default());
                assert_eq!(options.validation_commands, ["cargo test"]);
                Ok(supervise_placeholder_result(None))
            }
        }

        let service = CapturingSupervisor;
        service
            .supervise_task(
                &TaskId::parse(TASK_ID).unwrap(),
                SuperviseTaskOptions {
                    max_cycles: Some(2),
                    ticket_model: Some("ticket-model".to_string()),
                    ..SuperviseTaskOptions::default()
                },
            )
            .unwrap();
        service
            .create_and_supervise_task(SuperviseCreateOptions::new(
                "Fix",
                "Goal",
                vec!["cargo test".to_string()],
            ))
            .unwrap();
    }
}
