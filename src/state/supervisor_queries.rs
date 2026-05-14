use crate::HarnessResult;
use crate::domain::{Run, RunId, RunStatus, TaskId, Ticket, TicketId, TicketResolution};
use crate::service::{SupervisorStateStore, SupervisorStateView, SupervisorTaskState};
use crate::state::{SqliteTaskStore, TaskStore};
use rusqlite::{OptionalExtension, params};

impl SqliteTaskStore {
    pub fn supervisor_task_state(&self, task_id: &TaskId) -> HarnessResult<SupervisorTaskState> {
        let task = self.get_task(task_id)?;
        let latest_run = self.latest_run_for_task(task_id)?;
        let tickets = self.list_tickets(Some(task_id), None)?;
        let unconsumed_resolution = self.latest_unconsumed_resolution(task_id)?;

        Ok(SupervisorTaskState {
            task,
            latest_run,
            tickets,
            unconsumed_resolution,
        })
    }

    pub fn latest_stuck_run_for_task(&self, task_id: &TaskId) -> HarnessResult<Option<Run>> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT * FROM runs
             WHERE task_id = ?1 AND status = ?2
             ORDER BY started_at DESC, id DESC
             LIMIT 1",
            params![task_id.as_str(), RunStatus::Stuck.as_str()],
            super::row_to_run,
        )
        .optional()
        .map_err(super::sql_err)
    }

    pub fn tickets_for_run(&self, run_id: &RunId) -> HarnessResult<Vec<Ticket>> {
        let conn = self.lock_conn()?;
        let run = super::get_run_with_conn(&conn, run_id)?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM tickets
                 WHERE run_id = ?1 AND task_id = ?2
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(super::sql_err)?;
        let rows = stmt
            .query_map(
                params![run_id.as_str(), run.task_id.as_str()],
                super::row_to_ticket,
            )
            .map_err(super::sql_err)?;
        super::collect_rows(rows)
    }
}

impl SupervisorStateView for SqliteTaskStore {
    fn supervisor_task_state(&self, task_id: &TaskId) -> HarnessResult<SupervisorTaskState> {
        SqliteTaskStore::supervisor_task_state(self, task_id)
    }

    fn latest_stuck_run(&self, task_id: &TaskId) -> HarnessResult<Option<Run>> {
        self.latest_stuck_run_for_task(task_id)
    }

    fn tickets_for_run(&self, run_id: &RunId) -> HarnessResult<Vec<Ticket>> {
        SqliteTaskStore::tickets_for_run(self, run_id)
    }

    fn latest_unconsumed_resolution_for_ticket(
        &self,
        ticket_id: &TicketId,
    ) -> HarnessResult<Option<TicketResolution>> {
        TaskStore::latest_unconsumed_resolution_for_ticket(self, ticket_id)
    }
}

impl SupervisorStateStore for SqliteTaskStore {
    fn recover_expired_supervisor_leases(&self, task_id: &TaskId) -> HarnessResult<()> {
        self.get_task(task_id)?;
        self.recover_expired_leases(&super::now_string())?;
        Ok(())
    }

    fn mark_supervisor_cycle_started(&self, task_id: &TaskId, _cycle: u32) -> HarnessResult<()> {
        self.get_task(task_id).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{RunStatus, Task, TaskStatus, TicketStatus};

    const OWNER: &str = "supervisor-query-test";
    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const OTHER_TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const RUN_1: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_2: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const RUN_3: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAX";
    const TICKET_1: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_2: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAW";

    #[test]
    fn latest_stuck_run_ordering_uses_started_at_then_run_id() {
        let store = seeded_store(TaskStatus::Stuck);
        insert_run(&store, RUN_1, RunStatus::Stuck, "2026-01-01T00:00:01Z", 0);
        insert_run(&store, RUN_2, RunStatus::Stuck, "2026-01-01T00:00:02Z", 1);
        insert_run(&store, RUN_3, RunStatus::Stuck, "2026-01-01T00:00:02Z", 2);

        let latest = store
            .latest_stuck_run_for_task(&task_id())
            .unwrap()
            .unwrap();

        assert_eq!(latest.id.as_str(), RUN_3);
        assert_eq!(latest.escalation_cycle, 2);
    }

    #[test]
    fn latest_stuck_run_ignores_other_statuses_and_tasks() {
        let store = seeded_store(TaskStatus::Stuck);
        insert_run(&store, RUN_1, RunStatus::Stuck, "2026-01-01T00:00:01Z", 0);
        insert_run(
            &store,
            RUN_2,
            RunStatus::Complete,
            "2026-01-01T00:00:03Z",
            0,
        );
        insert_other_task_and_run(&store, "2026-01-01T00:00:04Z");

        let latest = store
            .latest_stuck_run_for_task(&task_id())
            .unwrap()
            .unwrap();

        assert_eq!(latest.id.as_str(), RUN_1);
    }

    #[test]
    fn tickets_for_run_is_run_and_task_scoped_with_stable_ordering() {
        let store = seeded_store(TaskStatus::Stuck);
        insert_run(&store, RUN_1, RunStatus::Stuck, "2026-01-01T00:00:01Z", 0);
        insert_run(&store, RUN_2, RunStatus::Stuck, "2026-01-01T00:00:02Z", 1);
        insert_ticket(
            &store,
            TICKET_2,
            RUN_1,
            TicketStatus::Open,
            "2026-01-01T00:00:05Z",
            "two",
        );
        insert_ticket(
            &store,
            TICKET_1,
            RUN_1,
            TicketStatus::Open,
            "2026-01-01T00:00:05Z",
            "one",
        );
        insert_ticket(
            &store,
            "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAX",
            RUN_2,
            TicketStatus::Open,
            "2026-01-01T00:00:06Z",
            "other-run",
        );

        let tickets = store
            .tickets_for_run(&RunId::parse(RUN_1).unwrap())
            .unwrap();

        assert_eq!(
            tickets
                .iter()
                .map(|ticket| ticket.id.as_str())
                .collect::<Vec<_>>(),
            vec![TICKET_1, TICKET_2]
        );
    }

    #[test]
    fn supervisor_task_state_is_read_only_and_preserves_lease() {
        let store = seeded_store(TaskStatus::Stuck);
        insert_run(&store, RUN_1, RunStatus::Stuck, "2026-01-01T00:00:01Z", 0);
        insert_ticket(
            &store,
            TICKET_1,
            RUN_1,
            TicketStatus::Open,
            "2026-01-01T00:00:05Z",
            "one",
        );

        let state = store.supervisor_task_state(&task_id()).unwrap();
        let task = store.get_task(&task_id()).unwrap();

        assert_eq!(state.latest_run.unwrap().id.as_str(), RUN_1);
        assert_eq!(state.tickets.len(), 1);
        assert_eq!(task.lease_owner.as_deref(), Some(OWNER));
    }

    #[test]
    fn recover_expired_supervisor_leases_uses_existing_store_semantics() {
        let store = SqliteTaskStore::in_memory().unwrap();
        let mut task = task(TaskStatus::Stuck);
        task.lease_owner = Some("expired-owner".to_string());
        task.lease_acquired_at = Some("1".to_string());
        task.lease_expires_at = Some("1".to_string());
        task.heartbeat_at = Some("1".to_string());
        store
            .insert_task(task.clone(), vec!["cargo test".to_string()])
            .unwrap();

        store.recover_expired_supervisor_leases(&task.id).unwrap();

        let recovered = store.get_task(&task.id).unwrap();
        assert_eq!(recovered.status, TaskStatus::Stuck);
        assert_eq!(recovered.lease_owner, None);
        assert_eq!(recovered.lease_expires_at, None);
    }

    fn seeded_store(status: TaskStatus) -> SqliteTaskStore {
        let store = SqliteTaskStore::in_memory().unwrap();
        let task = task(status);
        store
            .insert_task(task.clone(), vec!["cargo test".to_string()])
            .unwrap();
        store.acquire_task_lease(&task.id, OWNER).unwrap();
        store
    }

    fn task_id() -> TaskId {
        TaskId::parse(TASK_ID).unwrap()
    }

    fn task(status: TaskStatus) -> Task {
        Task {
            id: task_id(),
            title: "Fix tests".to_string(),
            goal: "Make cargo test pass".to_string(),
            status,
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

    fn insert_run(
        store: &SqliteTaskStore,
        id: &str,
        status: RunStatus,
        started_at: &str,
        escalation_cycle: u32,
    ) {
        store
            .insert_run(
                Run {
                    id: RunId::parse(id).unwrap(),
                    task_id: task_id(),
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
                },
                OWNER,
            )
            .unwrap();
    }

    fn insert_ticket(
        store: &SqliteTaskStore,
        id: &str,
        run_id: &str,
        status: TicketStatus,
        created_at: &str,
        fingerprint: &str,
    ) {
        store
            .insert_ticket(
                Ticket {
                    id: TicketId::parse(id).unwrap(),
                    task_id: task_id(),
                    run_id: RunId::parse(run_id).unwrap(),
                    status,
                    blocked_on: "validation".to_string(),
                    question: "What next?".to_string(),
                    reason: "stuck".to_string(),
                    evidence_json: "{}".to_string(),
                    failure_fingerprint: fingerprint.to_string(),
                    created_at: created_at.to_string(),
                    resolved_at: None,
                },
                OWNER,
            )
            .unwrap();
    }

    fn insert_other_task_and_run(store: &SqliteTaskStore, started_at: &str) {
        let mut other = task(TaskStatus::Stuck);
        other.id = TaskId::parse(OTHER_TASK_ID).unwrap();
        store
            .insert_task(other.clone(), vec!["cargo test".to_string()])
            .unwrap();
        store.acquire_task_lease(&other.id, "other-owner").unwrap();
        store
            .insert_run(
                Run {
                    id: RunId::parse("run_01ARZ3NDEKTSV4RRFFQ69G5FAY").unwrap(),
                    task_id: other.id,
                    parent_run_id: None,
                    status: RunStatus::Stuck,
                    repo_root: "/repo".to_string(),
                    base_ref: Some("main".to_string()),
                    base_commit: "abcdef".to_string(),
                    dirty_state_summary: None,
                    current_phase: Some("stuck".to_string()),
                    escalation_cycle: 9,
                    started_at: started_at.to_string(),
                    finished_at: Some(started_at.to_string()),
                    final_diff_path: None,
                    last_error: None,
                },
                "other-owner",
            )
            .unwrap();
    }
}
