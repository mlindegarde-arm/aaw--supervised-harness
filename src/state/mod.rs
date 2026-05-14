pub mod supervisor_queries;

use crate::domain::{
    Artifact, Attempt, AttemptId, Event, Run, RunId, RunStatus, Task, TaskId, TaskStatus,
    TaskValidationCommand, Ticket, TicketId, TicketResolution, TicketResolutionId, TicketStatus,
};
use crate::service::SupervisorStateStore;
use crate::{HarnessError, HarnessResult};
use rusqlite::types::{Type, ValueRef};
use rusqlite::{Connection, OptionalExtension, params};
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub task_id: TaskId,
    pub owner: String,
    pub acquired_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketFingerprint {
    pub blocked_on: String,
    pub failure_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunUpdate {
    pub status: Option<RunStatus>,
    pub repo_root: Option<String>,
    pub base_ref: Option<String>,
    pub base_commit: Option<String>,
    pub dirty_state_summary: Option<String>,
    pub current_phase: Option<String>,
    pub finished_at: Option<String>,
    pub final_diff_path: Option<String>,
    pub last_error: Option<String>,
}

pub trait TaskStore: SupervisorStateStore {
    fn insert_task(&self, task: Task, validation_commands: Vec<String>) -> HarnessResult<()>;
    fn list_tasks(&self, status: Option<TaskStatus>) -> HarnessResult<Vec<Task>>;
    fn get_task(&self, task_id: &TaskId) -> HarnessResult<Task>;
    fn update_task(&self, task: Task, lease_owner: &str) -> HarnessResult<()>;
    fn transition_task(
        &self,
        task_id: &TaskId,
        from: TaskStatus,
        to: TaskStatus,
        lease_owner: &str,
    ) -> HarnessResult<Task>;
    fn list_validation_commands(
        &self,
        task_id: &TaskId,
    ) -> HarnessResult<Vec<TaskValidationCommand>>;
    fn insert_run(&self, run: Run, lease_owner: &str) -> HarnessResult<()>;
    fn get_run(&self, run_id: &RunId) -> HarnessResult<Run>;
    fn latest_run_for_task(&self, task_id: &TaskId) -> HarnessResult<Option<Run>>;
    fn update_run(
        &self,
        run_id: &RunId,
        expected: Option<RunStatus>,
        update: RunUpdate,
        lease_owner: &str,
    ) -> HarnessResult<Run>;
    fn insert_attempt(&self, attempt: Attempt, lease_owner: &str) -> HarnessResult<()>;
    fn list_attempts(&self, run_id: &RunId) -> HarnessResult<Vec<Attempt>>;
    fn insert_ticket(&self, ticket: Ticket, lease_owner: &str) -> HarnessResult<()>;
    fn create_or_get_ticket(&self, ticket: Ticket, lease_owner: &str) -> HarnessResult<Ticket>;
    fn get_ticket(&self, ticket_id: &TicketId) -> HarnessResult<Ticket>;
    fn list_tickets(
        &self,
        task_id: Option<&TaskId>,
        status: Option<TicketStatus>,
    ) -> HarnessResult<Vec<Ticket>>;
    fn transition_ticket(
        &self,
        ticket_id: &TicketId,
        from: TicketStatus,
        to: TicketStatus,
        lease_owner: &str,
    ) -> HarnessResult<Ticket>;
    fn insert_ticket_resolution(
        &self,
        resolution: TicketResolution,
        lease_owner: &str,
    ) -> HarnessResult<()>;
    fn get_ticket_resolution(
        &self,
        resolution_id: &TicketResolutionId,
    ) -> HarnessResult<TicketResolution>;
    fn list_ticket_resolutions(&self, ticket_id: &TicketId)
    -> HarnessResult<Vec<TicketResolution>>;
    fn latest_unconsumed_resolution_for_ticket(
        &self,
        ticket_id: &TicketId,
    ) -> HarnessResult<Option<TicketResolution>>;
    fn latest_unconsumed_resolution_for_run(
        &self,
        run_id: &RunId,
    ) -> HarnessResult<Option<TicketResolution>>;
    fn latest_unconsumed_resolution(
        &self,
        task_id: &TaskId,
    ) -> HarnessResult<Option<TicketResolution>>;
    fn mark_ticket_resolution_consumed(
        &self,
        resolution_id: &TicketResolutionId,
        consumed_at: &str,
        lease_owner: &str,
    ) -> HarnessResult<()>;
    fn insert_artifact(&self, artifact: Artifact, lease_owner: &str) -> HarnessResult<()>;
    fn list_artifacts_for_run(&self, run_id: &RunId) -> HarnessResult<Vec<Artifact>>;
    fn insert_event(&self, event: Event, lease_owner: &str) -> HarnessResult<()>;
    fn list_events_for_task(&self, task_id: &TaskId) -> HarnessResult<Vec<Event>>;
    fn acquire_task_lease(&self, task_id: &TaskId, owner: &str) -> HarnessResult<Lease>;
    fn heartbeat_task_lease(&self, task_id: &TaskId, owner: &str) -> HarnessResult<()>;
    fn release_task_lease(&self, task_id: &TaskId, owner: &str) -> HarnessResult<()>;
    fn recover_expired_leases(&self, now: &str) -> HarnessResult<Vec<TaskId>>;
}

const DEFAULT_LEASE_TTL_SECS: i64 = 300;

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial_state_store",
    sql: r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at TEXT NOT NULL,
    checksum TEXT NOT NULL
);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    goal TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('ready', 'running', 'complete', 'stuck', 'failed')),
    repo_root TEXT NOT NULL,
    worktree_path TEXT,
    branch TEXT,
    base_ref TEXT,
    base_commit TEXT,
    last_seen_head TEXT,
    max_attempts INTEGER NOT NULL,
    lease_owner TEXT,
    lease_acquired_at TEXT,
    lease_expires_at TEXT,
    heartbeat_at TEXT,
    lock_version INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE task_validation_commands (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    command TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (task_id, position)
);

CREATE TABLE runs (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    parent_run_id TEXT REFERENCES runs(id),
    status TEXT NOT NULL CHECK (status IN ('running', 'complete', 'stuck', 'failed')),
    repo_root TEXT NOT NULL,
    base_ref TEXT,
    base_commit TEXT NOT NULL,
    dirty_state_summary TEXT,
    current_phase TEXT,
    escalation_cycle INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    final_diff_path TEXT,
    last_error TEXT
);

CREATE TABLE attempts (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'complete', 'failed', 'invalid_response', 'patch_rejected', 'validation_failed')),
    prompt_path TEXT,
    response_path TEXT,
    patch_path TEXT,
    validation_log_path TEXT,
    validation_exit_code INTEGER,
    failure_reason TEXT,
    apply_error TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT
);

CREATE TABLE tickets (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('open', 'resolving', 'resolved', 'failed')),
    blocked_on TEXT NOT NULL,
    question TEXT NOT NULL,
    reason TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    failure_fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved_at TEXT,
    UNIQUE(task_id, run_id, blocked_on, failure_fingerprint)
);

CREATE TABLE ticket_resolutions (
    id TEXT PRIMARY KEY,
    ticket_id TEXT NOT NULL REFERENCES tickets(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    response_id TEXT,
    resolution_path TEXT NOT NULL,
    consumed_at TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE CASCADE,
    attempt_id TEXT REFERENCES attempts(id) ON DELETE CASCADE,
    ticket_id TEXT REFERENCES tickets(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    byte_len INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE events (
    id TEXT PRIMARY KEY,
    task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    level TEXT NOT NULL CHECK (level IN ('info', 'warn', 'error')),
    message TEXT NOT NULL,
    artifact_path TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_tasks_status_created_at ON tasks(status, created_at);
CREATE INDEX idx_runs_task_started_at ON runs(task_id, started_at);
CREATE INDEX idx_attempts_run_attempt_number ON attempts(run_id, attempt_number);
CREATE INDEX idx_tickets_status_created_at ON tickets(status, created_at);
CREATE INDEX idx_tickets_task_status ON tickets(task_id, status);
CREATE INDEX idx_ticket_resolutions_ticket_created_at ON ticket_resolutions(ticket_id, created_at);
CREATE INDEX idx_events_task_created_at ON events(task_id, created_at);
"#,
}];

#[derive(Debug, Clone, Copy)]
struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

pub struct SqliteTaskStore {
    conn: Mutex<Connection>,
    lease_ttl_secs: i64,
}

impl SqliteTaskStore {
    pub fn open(path: impl AsRef<Path>) -> HarnessResult<Self> {
        if let Some(parent) = path.as_ref().parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|err| HarnessError::External(format!("create state dir: {err}")))?;
        }
        let conn = Connection::open(path).map_err(sql_err)?;
        initialize_connection(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            lease_ttl_secs: DEFAULT_LEASE_TTL_SECS,
        })
    }

    pub fn in_memory() -> HarnessResult<Self> {
        let conn = Connection::open_in_memory().map_err(sql_err)?;
        initialize_connection(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            lease_ttl_secs: DEFAULT_LEASE_TTL_SECS,
        })
    }

    pub fn with_lease_ttl_secs(mut self, lease_ttl_secs: i64) -> Self {
        self.lease_ttl_secs = lease_ttl_secs;
        self
    }

    pub fn pragma_value(&self, name: &str) -> HarnessResult<String> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(&format!("PRAGMA {name}")).map_err(sql_err)?;
        stmt.query_row([], |row| match row.get_ref(0)? {
            ValueRef::Null => Ok(String::new()),
            ValueRef::Integer(value) => Ok(value.to_string()),
            ValueRef::Real(value) => Ok(value.to_string()),
            ValueRef::Text(value) => Ok(String::from_utf8_lossy(value).into_owned()),
            ValueRef::Blob(value) => Ok(format!("{value:x?}")),
        })
        .map_err(sql_err)
    }

    fn lock_conn(&self) -> HarnessResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| HarnessError::External("sqlite connection lock poisoned".to_string()))
    }
}

impl TaskStore for SqliteTaskStore {
    fn insert_task(&self, task: Task, validation_commands: Vec<String>) -> HarnessResult<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        insert_task_row(&tx, &task)?;
        for (idx, command) in validation_commands.iter().enumerate() {
            tx.execute(
                "INSERT INTO task_validation_commands (task_id, position, command, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![task.id.as_str(), idx as i64, command, task.created_at],
            )
            .map_err(sql_err)?;
        }
        tx.commit().map_err(sql_err)
    }

    fn list_tasks(&self, status: Option<TaskStatus>) -> HarnessResult<Vec<Task>> {
        let conn = self.lock_conn()?;
        let sql = match status {
            Some(_) => "SELECT * FROM tasks WHERE status = ?1 ORDER BY created_at ASC, id ASC",
            None => "SELECT * FROM tasks ORDER BY created_at ASC, id ASC",
        };
        let mut stmt = conn.prepare(sql).map_err(sql_err)?;
        let rows = match status {
            Some(status) => stmt
                .query_map(params![status.as_str()], row_to_task)
                .map_err(sql_err)?,
            None => stmt.query_map([], row_to_task).map_err(sql_err)?,
        };
        collect_rows(rows)
    }

    fn get_task(&self, task_id: &TaskId) -> HarnessResult<Task> {
        let conn = self.lock_conn()?;
        get_task_with_conn(&conn, task_id)
    }

    fn update_task(&self, task: Task, lease_owner: &str) -> HarnessResult<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        assert_task_lease(&tx, &task.id, lease_owner)?;
        let rows = tx
            .execute(
                "UPDATE tasks
                 SET title = ?2, goal = ?3, repo_root = ?4, worktree_path = ?5,
                     branch = ?6, base_ref = ?7, base_commit = ?8, last_seen_head = ?9,
                     max_attempts = ?10, lock_version = lock_version + 1,
                     created_at = ?11, updated_at = ?12
                 WHERE id = ?1 AND lock_version = ?13",
                params![
                    task.id.as_str(),
                    task.title,
                    task.goal,
                    task.repo_root,
                    task.worktree_path,
                    task.branch,
                    task.base_ref,
                    task.base_commit,
                    task.last_seen_head,
                    task.max_attempts as i64,
                    task.created_at,
                    task.updated_at,
                    task.lock_version as i64,
                ],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?1)",
                    params![task.id.as_str()],
                    |row| row.get(0),
                )
                .map_err(sql_err)?;
            if exists {
                return Err(HarnessError::Conflict(format!(
                    "task {} lock_version changed",
                    task.id
                )));
            }
            return Err(not_found("task", task.id.as_str()));
        }
        tx.commit().map_err(sql_err)
    }

    fn transition_task(
        &self,
        task_id: &TaskId,
        from: TaskStatus,
        to: TaskStatus,
        lease_owner: &str,
    ) -> HarnessResult<Task> {
        if !valid_task_transition(from, to) {
            return Err(HarnessError::Conflict(format!(
                "invalid task status transition {from} -> {to}"
            )));
        }
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        assert_task_lease(&tx, task_id, lease_owner)?;
        if from == TaskStatus::Stuck
            && to == TaskStatus::Running
            && !has_resolved_unconsumed_resolution_for_task(&tx, task_id)?
        {
            return Err(HarnessError::Conflict(format!(
                "task {} cannot resume without a resolved unconsumed ticket resolution",
                task_id
            )));
        }
        let rows = tx
            .execute(
                "UPDATE tasks
                 SET status = ?2, lock_version = lock_version + 1, updated_at = ?3
                 WHERE id = ?1 AND status = ?4",
                params![task_id.as_str(), to.as_str(), now_string(), from.as_str()],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            ensure_exists(&tx, "tasks", task_id.as_str(), "task")?;
            return Err(HarnessError::Conflict(format!(
                "task {} was not in status {from}",
                task_id
            )));
        }
        let task = get_task_with_conn(&tx, task_id)?;
        tx.commit().map_err(sql_err)?;
        Ok(task)
    }

    fn list_validation_commands(
        &self,
        task_id: &TaskId,
    ) -> HarnessResult<Vec<TaskValidationCommand>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT task_id, position, command, created_at
                 FROM task_validation_commands
                 WHERE task_id = ?1
                 ORDER BY position ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![task_id.as_str()], row_to_validation_command)
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn insert_run(&self, run: Run, lease_owner: &str) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        assert_task_lease(&conn, &run.task_id, lease_owner)?;
        assert_parent_run_matches_task(&conn, &run)?;
        conn.execute(
            "INSERT INTO runs
             (id, task_id, parent_run_id, status, repo_root, base_ref, base_commit,
              dirty_state_summary, current_phase, escalation_cycle, started_at, finished_at,
              final_diff_path, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                run.id.as_str(),
                run.task_id.as_str(),
                run.parent_run_id.as_ref().map(RunId::as_str),
                run.status.as_str(),
                run.repo_root,
                run.base_ref,
                run.base_commit,
                run.dirty_state_summary,
                run.current_phase,
                run.escalation_cycle as i64,
                run.started_at,
                run.finished_at,
                run.final_diff_path,
                run.last_error,
            ],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    fn get_run(&self, run_id: &RunId) -> HarnessResult<Run> {
        let conn = self.lock_conn()?;
        get_run_with_conn(&conn, run_id)
    }

    fn latest_run_for_task(&self, task_id: &TaskId) -> HarnessResult<Option<Run>> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT * FROM runs WHERE task_id = ?1 ORDER BY started_at DESC, id DESC LIMIT 1",
            params![task_id.as_str()],
            row_to_run,
        )
        .optional()
        .map_err(sql_err)
    }

    fn update_run(
        &self,
        run_id: &RunId,
        expected: Option<RunStatus>,
        update: RunUpdate,
        lease_owner: &str,
    ) -> HarnessResult<Run> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        let mut run = get_run_with_conn(&tx, run_id)?;
        assert_task_lease(&tx, &run.task_id, lease_owner)?;
        if let Some(expected) = expected
            && run.status != expected
        {
            return Err(HarnessError::Conflict(format!(
                "run {} was not in status {expected}",
                run_id
            )));
        }
        if let Some(status) = update.status {
            if !valid_run_transition(run.status, status) {
                return Err(HarnessError::Conflict(format!(
                    "invalid run status transition {} -> {status}",
                    run.status
                )));
            }
            run.status = status;
        }
        apply_run_update(&mut run, update);
        tx.execute(
            "UPDATE runs
             SET task_id = ?2, parent_run_id = ?3, status = ?4, repo_root = ?5, base_ref = ?6,
                 base_commit = ?7, dirty_state_summary = ?8, current_phase = ?9,
                 escalation_cycle = ?10, started_at = ?11, finished_at = ?12,
                 final_diff_path = ?13, last_error = ?14
             WHERE id = ?1",
            params![
                run.id.as_str(),
                run.task_id.as_str(),
                run.parent_run_id.as_ref().map(RunId::as_str),
                run.status.as_str(),
                run.repo_root,
                run.base_ref,
                run.base_commit,
                run.dirty_state_summary,
                run.current_phase,
                run.escalation_cycle as i64,
                run.started_at,
                run.finished_at,
                run.final_diff_path,
                run.last_error,
            ],
        )
        .map_err(sql_err)?;
        tx.commit().map_err(sql_err)?;
        Ok(run)
    }

    fn insert_attempt(&self, attempt: Attempt, lease_owner: &str) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        let run = get_run_with_conn(&conn, &attempt.run_id)?;
        assert_task_lease(&conn, &run.task_id, lease_owner)?;
        conn.execute(
            "INSERT INTO attempts
             (id, run_id, attempt_number, provider, model, status, prompt_path, response_path,
              patch_path, validation_log_path, validation_exit_code, failure_reason, apply_error,
              started_at, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                attempt.id.as_str(),
                attempt.run_id.as_str(),
                attempt.attempt_number as i64,
                attempt.provider,
                attempt.model,
                attempt.status.as_str(),
                attempt.prompt_path,
                attempt.response_path,
                attempt.patch_path,
                attempt.validation_log_path,
                attempt.validation_exit_code,
                attempt.failure_reason,
                attempt.apply_error,
                attempt.started_at,
                attempt.finished_at,
            ],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    fn list_attempts(&self, run_id: &RunId) -> HarnessResult<Vec<Attempt>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM attempts WHERE run_id = ?1 ORDER BY attempt_number ASC, id ASC")
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![run_id.as_str()], row_to_attempt)
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn insert_ticket(&self, ticket: Ticket, lease_owner: &str) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        assert_ticket_matches_run_and_lease(&conn, &ticket, lease_owner)?;
        insert_ticket_row(&conn, &ticket)
    }

    fn create_or_get_ticket(&self, ticket: Ticket, lease_owner: &str) -> HarnessResult<Ticket> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        assert_ticket_matches_run_and_lease(&tx, &ticket, lease_owner)?;
        match insert_ticket_row(&tx, &ticket) {
            Ok(()) => {
                tx.commit().map_err(sql_err)?;
                Ok(ticket)
            }
            Err(HarnessError::Conflict(_)) => {
                let existing = tx
                    .query_row(
                        "SELECT * FROM tickets
                         WHERE task_id = ?1 AND run_id = ?2 AND blocked_on = ?3 AND failure_fingerprint = ?4",
                        params![
                            ticket.task_id.as_str(),
                            ticket.run_id.as_str(),
                            ticket.blocked_on,
                            ticket.failure_fingerprint,
                        ],
                        row_to_ticket,
                    )
                    .optional()
                    .map_err(sql_err)?;
                match existing {
                    Some(existing) => {
                        tx.commit().map_err(sql_err)?;
                        Ok(existing)
                    }
                    None => Err(HarnessError::Conflict(
                        "ticket insert failed and no existing fingerprint match was found"
                            .to_string(),
                    )),
                }
            }
            Err(err) => Err(err),
        }
    }

    fn get_ticket(&self, ticket_id: &TicketId) -> HarnessResult<Ticket> {
        let conn = self.lock_conn()?;
        get_ticket_with_conn(&conn, ticket_id)
    }

    fn list_tickets(
        &self,
        task_id: Option<&TaskId>,
        status: Option<TicketStatus>,
    ) -> HarnessResult<Vec<Ticket>> {
        let conn = self.lock_conn()?;
        let (sql, task_arg, status_arg) = match (task_id, status) {
            (Some(task_id), Some(status)) => (
                "SELECT * FROM tickets WHERE task_id = ?1 AND status = ?2 ORDER BY created_at ASC, id ASC",
                Some(task_id.as_str()),
                Some(status.as_str()),
            ),
            (Some(task_id), None) => (
                "SELECT * FROM tickets WHERE task_id = ?1 ORDER BY created_at ASC, id ASC",
                Some(task_id.as_str()),
                None,
            ),
            (None, Some(status)) => (
                "SELECT * FROM tickets WHERE status = ?1 ORDER BY created_at ASC, id ASC",
                None,
                Some(status.as_str()),
            ),
            (None, None) => (
                "SELECT * FROM tickets ORDER BY created_at ASC, id ASC",
                None,
                None,
            ),
        };
        let mut stmt = conn.prepare(sql).map_err(sql_err)?;
        let tickets = match (task_arg, status_arg) {
            (Some(task_id), Some(status)) => stmt
                .query_map(params![task_id, status], row_to_ticket)
                .map_err(sql_err)?,
            (Some(task_id), None) => stmt
                .query_map(params![task_id], row_to_ticket)
                .map_err(sql_err)?,
            (None, Some(status)) => stmt
                .query_map(params![status], row_to_ticket)
                .map_err(sql_err)?,
            (None, None) => stmt.query_map([], row_to_ticket).map_err(sql_err)?,
        };
        collect_rows(tickets)
    }

    fn transition_ticket(
        &self,
        ticket_id: &TicketId,
        from: TicketStatus,
        to: TicketStatus,
        lease_owner: &str,
    ) -> HarnessResult<Ticket> {
        if !valid_ticket_transition(from, to) {
            return Err(HarnessError::Conflict(format!(
                "invalid ticket status transition {from} -> {to}"
            )));
        }
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        let current = get_ticket_with_conn(&tx, ticket_id)?;
        assert_task_lease(&tx, &current.task_id, lease_owner)?;
        if to == TicketStatus::Resolved && !has_ticket_resolution(&tx, ticket_id)? {
            return Err(HarnessError::Conflict(format!(
                "ticket {} cannot be resolved without a resolution row",
                ticket_id
            )));
        }
        let resolved_at = if to == TicketStatus::Resolved {
            Some(now_string())
        } else {
            None
        };
        let rows = tx
            .execute(
                "UPDATE tickets
                 SET status = ?2, resolved_at = COALESCE(?3, resolved_at)
                 WHERE id = ?1 AND status = ?4",
                params![ticket_id.as_str(), to.as_str(), resolved_at, from.as_str()],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            ensure_exists(&tx, "tickets", ticket_id.as_str(), "ticket")?;
            return Err(HarnessError::Conflict(format!(
                "ticket {} was not in status {from}",
                ticket_id
            )));
        }
        let ticket = get_ticket_with_conn(&tx, ticket_id)?;
        tx.commit().map_err(sql_err)?;
        Ok(ticket)
    }

    fn insert_ticket_resolution(
        &self,
        resolution: TicketResolution,
        lease_owner: &str,
    ) -> HarnessResult<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        let ticket = get_ticket_with_conn(&tx, &resolution.ticket_id)?;
        assert_task_lease(&tx, &ticket.task_id, lease_owner)?;
        if ticket.status != TicketStatus::Resolving {
            return Err(HarnessError::Conflict(format!(
                "ticket {} must be resolving before resolution insertion",
                ticket.id
            )));
        }
        tx.execute(
            "INSERT INTO ticket_resolutions
             (id, ticket_id, provider, model, response_id, resolution_path, consumed_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                resolution.id.as_str(),
                resolution.ticket_id.as_str(),
                resolution.provider,
                resolution.model,
                resolution.response_id,
                resolution.resolution_path,
                resolution.consumed_at,
                resolution.created_at,
            ],
        )
        .map_err(sql_err)?;
        tx.execute(
            "UPDATE tickets SET status = 'resolved', resolved_at = ?2 WHERE id = ?1",
            params![resolution.ticket_id.as_str(), now_string()],
        )
        .map_err(sql_err)?;
        tx.commit().map_err(sql_err)?;
        Ok(())
    }

    fn get_ticket_resolution(
        &self,
        resolution_id: &TicketResolutionId,
    ) -> HarnessResult<TicketResolution> {
        let conn = self.lock_conn()?;
        get_ticket_resolution_with_conn(&conn, resolution_id)
    }

    fn list_ticket_resolutions(
        &self,
        ticket_id: &TicketId,
    ) -> HarnessResult<Vec<TicketResolution>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM ticket_resolutions
                 WHERE ticket_id = ?1
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![ticket_id.as_str()], row_to_ticket_resolution)
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn latest_unconsumed_resolution_for_ticket(
        &self,
        ticket_id: &TicketId,
    ) -> HarnessResult<Option<TicketResolution>> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT * FROM ticket_resolutions
             WHERE ticket_id = ?1
               AND consumed_at IS NULL
               AND EXISTS (
                   SELECT 1 FROM tickets
                   WHERE tickets.id = ticket_resolutions.ticket_id
                     AND tickets.status = 'resolved'
               )
             ORDER BY created_at DESC, id DESC
             LIMIT 1",
            params![ticket_id.as_str()],
            row_to_ticket_resolution,
        )
        .optional()
        .map_err(sql_err)
    }

    fn latest_unconsumed_resolution_for_run(
        &self,
        run_id: &RunId,
    ) -> HarnessResult<Option<TicketResolution>> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT tr.*
             FROM ticket_resolutions tr
             JOIN tickets t ON t.id = tr.ticket_id
             WHERE t.run_id = ?1 AND t.status = 'resolved' AND tr.consumed_at IS NULL
             ORDER BY tr.created_at DESC, tr.id DESC
             LIMIT 1",
            params![run_id.as_str()],
            row_to_ticket_resolution,
        )
        .optional()
        .map_err(sql_err)
    }

    fn latest_unconsumed_resolution(
        &self,
        task_id: &TaskId,
    ) -> HarnessResult<Option<TicketResolution>> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT tr.*
             FROM ticket_resolutions tr
             JOIN tickets t ON t.id = tr.ticket_id
             WHERE t.task_id = ?1 AND t.status = 'resolved' AND tr.consumed_at IS NULL
             ORDER BY tr.created_at DESC, tr.id DESC
             LIMIT 1",
            params![task_id.as_str()],
            row_to_ticket_resolution,
        )
        .optional()
        .map_err(sql_err)
    }

    fn mark_ticket_resolution_consumed(
        &self,
        resolution_id: &TicketResolutionId,
        consumed_at: &str,
        lease_owner: &str,
    ) -> HarnessResult<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        let resolution = get_ticket_resolution_with_conn(&tx, resolution_id)?;
        let ticket = get_ticket_with_conn(&tx, &resolution.ticket_id)?;
        assert_task_lease(&tx, &ticket.task_id, lease_owner)?;
        if ticket.status != TicketStatus::Resolved {
            return Err(HarnessError::Conflict(format!(
                "ticket {} is not resolved",
                ticket.id
            )));
        }
        let rows = tx
            .execute(
                "UPDATE ticket_resolutions
                 SET consumed_at = ?2
                 WHERE id = ?1 AND consumed_at IS NULL",
                params![resolution_id.as_str(), consumed_at],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            ensure_exists(
                &tx,
                "ticket_resolutions",
                resolution_id.as_str(),
                "ticket_resolution",
            )?;
        }
        tx.commit().map_err(sql_err)?;
        Ok(())
    }

    fn insert_artifact(&self, artifact: Artifact, lease_owner: &str) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        assert_artifact_matches_references_and_lease(&conn, &artifact, lease_owner)?;
        conn.execute(
            "INSERT INTO artifacts
             (id, task_id, run_id, attempt_id, ticket_id, kind, path, sha256, byte_len, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                artifact.id.as_str(),
                artifact.task_id.as_str(),
                artifact.run_id.as_ref().map(RunId::as_str),
                artifact.attempt_id.as_ref().map(AttemptId::as_str),
                artifact.ticket_id.as_ref().map(TicketId::as_str),
                artifact.kind,
                artifact.path,
                artifact.sha256,
                artifact.byte_len as i64,
                artifact.created_at,
            ],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    fn list_artifacts_for_run(&self, run_id: &RunId) -> HarnessResult<Vec<Artifact>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM artifacts WHERE run_id = ?1 ORDER BY created_at ASC, id ASC")
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![run_id.as_str()], row_to_artifact)
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn insert_event(&self, event: Event, lease_owner: &str) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        assert_event_matches_references_and_lease(&conn, &event, lease_owner)?;
        conn.execute(
            "INSERT INTO events
             (id, task_id, run_id, kind, level, message, artifact_path, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.id.as_str(),
                event.task_id.as_ref().map(TaskId::as_str),
                event.run_id.as_ref().map(RunId::as_str),
                event.kind,
                event.level.as_str(),
                event.message,
                event.artifact_path,
                event.created_at,
            ],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    fn list_events_for_task(&self, task_id: &TaskId) -> HarnessResult<Vec<Event>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM events WHERE task_id = ?1 ORDER BY created_at ASC, id ASC")
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![task_id.as_str()], row_to_event)
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn acquire_task_lease(&self, task_id: &TaskId, owner: &str) -> HarnessResult<Lease> {
        let now = current_unix_secs();
        let acquired_at = now.to_string();
        let expires_at = (now + self.lease_ttl_secs).to_string();
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        ensure_exists(&tx, "tasks", task_id.as_str(), "task")?;
        let rows = tx
            .execute(
                "UPDATE tasks
                 SET lease_owner = ?2, lease_acquired_at = ?3, lease_expires_at = ?4,
                     heartbeat_at = ?3, lock_version = lock_version + 1, updated_at = ?3
                 WHERE id = ?1
                   AND (lease_owner IS NULL OR lease_expires_at IS NULL OR CAST(lease_expires_at AS INTEGER) <= ?5)",
                params![task_id.as_str(), owner, acquired_at, expires_at, now],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            return Err(HarnessError::Conflict(format!(
                "task {} already has a non-expired lease",
                task_id
            )));
        }
        tx.commit().map_err(sql_err)?;
        Ok(Lease {
            task_id: task_id.clone(),
            owner: owner.to_string(),
            acquired_at,
            expires_at,
        })
    }

    fn heartbeat_task_lease(&self, task_id: &TaskId, owner: &str) -> HarnessResult<()> {
        let now = current_unix_secs();
        let heartbeat_at = now.to_string();
        let expires_at = (now + self.lease_ttl_secs).to_string();
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "UPDATE tasks
                 SET heartbeat_at = ?3, lease_expires_at = ?4, lock_version = lock_version + 1,
                     updated_at = ?3
                 WHERE id = ?1 AND lease_owner = ?2 AND CAST(lease_expires_at AS INTEGER) > ?5",
                params![task_id.as_str(), owner, heartbeat_at, expires_at, now],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            ensure_exists(&conn, "tasks", task_id.as_str(), "task")?;
            return Err(HarnessError::Conflict(format!(
                "task {} lease is not held by {owner} or has expired",
                task_id
            )));
        }
        Ok(())
    }

    fn release_task_lease(&self, task_id: &TaskId, owner: &str) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "UPDATE tasks
                 SET lease_owner = NULL, lease_acquired_at = NULL, lease_expires_at = NULL,
                     heartbeat_at = NULL, lock_version = lock_version + 1, updated_at = ?3
                 WHERE id = ?1 AND lease_owner = ?2",
                params![task_id.as_str(), owner, now_string()],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            ensure_exists(&conn, "tasks", task_id.as_str(), "task")?;
            return Err(HarnessError::Conflict(format!(
                "task {} lease is not held by {owner}",
                task_id
            )));
        }
        Ok(())
    }

    fn recover_expired_leases(&self, now: &str) -> HarnessResult<Vec<TaskId>> {
        let now_value = now.parse::<i64>().unwrap_or_else(|_| current_unix_secs());
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        let expired: Vec<TaskId> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id FROM tasks
                     WHERE lease_owner IS NOT NULL
                       AND lease_expires_at IS NOT NULL
                       AND CAST(lease_expires_at AS INTEGER) <= ?1
                     ORDER BY id ASC",
                )
                .map_err(sql_err)?;
            let rows = stmt
                .query_map(params![now_value], |row| {
                    parse_id(row.get::<_, String>(0)?, 0)
                })
                .map_err(sql_err)?;
            collect_rows(rows)?
        };

        for task_id in &expired {
            tx.execute(
                "UPDATE runs
                 SET status = 'failed', finished_at = ?2, last_error = 'lease expired'
                 WHERE task_id = ?1 AND status = 'running'",
                params![task_id.as_str(), now],
            )
            .map_err(sql_err)?;
            tx.execute(
                "UPDATE tasks
                 SET status = CASE WHEN status = 'running' THEN 'failed' ELSE status END,
                     lease_owner = NULL, lease_acquired_at = NULL, lease_expires_at = NULL,
                     heartbeat_at = NULL, lock_version = lock_version + 1, updated_at = ?2
                 WHERE id = ?1",
                params![task_id.as_str(), now],
            )
            .map_err(sql_err)?;
        }

        tx.commit().map_err(sql_err)?;
        Ok(expired)
    }
}

fn initialize_connection(conn: &Connection) -> HarnessResult<()> {
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sql_err)?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .map_err(sql_err)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(sql_err)?;
    run_migrations(conn)
}

fn run_migrations(conn: &Connection) -> HarnessResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL,
            checksum TEXT NOT NULL
        );",
    )
    .map_err(sql_err)?;

    for migration in MIGRATIONS {
        let checksum = checksum(migration.sql);
        let applied = conn
            .query_row(
                "SELECT name, checksum FROM schema_migrations WHERE version = ?1",
                params![migration.version],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(sql_err)?;

        match applied {
            Some((name, applied_checksum))
                if name == migration.name && applied_checksum == checksum => {}
            Some(_) => {
                return Err(HarnessError::Conflict(format!(
                    "migration {} metadata does not match embedded migration",
                    migration.version
                )));
            }
            None => {
                let tx = conn.unchecked_transaction().map_err(sql_err)?;
                tx.execute_batch(migration.sql).map_err(sql_err)?;
                tx.execute(
                    "INSERT INTO schema_migrations (version, name, applied_at, checksum)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![migration.version, migration.name, now_string(), checksum],
                )
                .map_err(sql_err)?;
                tx.commit().map_err(sql_err)?;
            }
        }
    }

    Ok(())
}

fn insert_task_row(conn: &Connection, task: &Task) -> HarnessResult<()> {
    conn.execute(
        "INSERT INTO tasks
         (id, title, goal, status, repo_root, worktree_path, branch, base_ref, base_commit,
          last_seen_head, max_attempts, lease_owner, lease_acquired_at, lease_expires_at,
          heartbeat_at, lock_version, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            task.id.as_str(),
            task.title,
            task.goal,
            task.status.as_str(),
            task.repo_root,
            task.worktree_path,
            task.branch,
            task.base_ref,
            task.base_commit,
            task.last_seen_head,
            task.max_attempts as i64,
            task.lease_owner,
            task.lease_acquired_at,
            task.lease_expires_at,
            task.heartbeat_at,
            task.lock_version as i64,
            task.created_at,
            task.updated_at,
        ],
    )
    .map_err(sql_err)?;
    Ok(())
}

fn insert_ticket_row(conn: &Connection, ticket: &Ticket) -> HarnessResult<()> {
    conn.execute(
        "INSERT INTO tickets
         (id, task_id, run_id, status, blocked_on, question, reason, evidence_json,
          failure_fingerprint, created_at, resolved_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            ticket.id.as_str(),
            ticket.task_id.as_str(),
            ticket.run_id.as_str(),
            ticket.status.as_str(),
            ticket.blocked_on,
            ticket.question,
            ticket.reason,
            ticket.evidence_json,
            ticket.failure_fingerprint,
            ticket.created_at,
            ticket.resolved_at,
        ],
    )
    .map_err(|err| match err {
        rusqlite::Error::SqliteFailure(sqlite_err, message)
            if sqlite_err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            HarnessError::Conflict(
                message.unwrap_or_else(|| "ticket constraint failed".to_string()),
            )
        }
        other => sql_err(other),
    })?;
    Ok(())
}

fn get_task_with_conn(conn: &Connection, task_id: &TaskId) -> HarnessResult<Task> {
    conn.query_row(
        "SELECT * FROM tasks WHERE id = ?1",
        params![task_id.as_str()],
        row_to_task,
    )
    .optional()
    .map_err(sql_err)?
    .ok_or_else(|| not_found("task", task_id.as_str()))
}

fn get_run_with_conn(conn: &Connection, run_id: &RunId) -> HarnessResult<Run> {
    conn.query_row(
        "SELECT * FROM runs WHERE id = ?1",
        params![run_id.as_str()],
        row_to_run,
    )
    .optional()
    .map_err(sql_err)?
    .ok_or_else(|| not_found("run", run_id.as_str()))
}

fn get_attempt_with_conn(conn: &Connection, attempt_id: &AttemptId) -> HarnessResult<Attempt> {
    conn.query_row(
        "SELECT * FROM attempts WHERE id = ?1",
        params![attempt_id.as_str()],
        row_to_attempt,
    )
    .optional()
    .map_err(sql_err)?
    .ok_or_else(|| not_found("attempt", attempt_id.as_str()))
}

fn get_ticket_with_conn(conn: &Connection, ticket_id: &TicketId) -> HarnessResult<Ticket> {
    conn.query_row(
        "SELECT * FROM tickets WHERE id = ?1",
        params![ticket_id.as_str()],
        row_to_ticket,
    )
    .optional()
    .map_err(sql_err)?
    .ok_or_else(|| not_found("ticket", ticket_id.as_str()))
}

fn get_ticket_resolution_with_conn(
    conn: &Connection,
    resolution_id: &TicketResolutionId,
) -> HarnessResult<TicketResolution> {
    conn.query_row(
        "SELECT * FROM ticket_resolutions WHERE id = ?1",
        params![resolution_id.as_str()],
        row_to_ticket_resolution,
    )
    .optional()
    .map_err(sql_err)?
    .ok_or_else(|| not_found("ticket_resolution", resolution_id.as_str()))
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: parse_id(row.get(0)?, 0)?,
        title: row.get(1)?,
        goal: row.get(2)?,
        status: parse_status(row.get::<_, String>(3)?, 3)?,
        repo_root: row.get(4)?,
        worktree_path: row.get(5)?,
        branch: row.get(6)?,
        base_ref: row.get(7)?,
        base_commit: row.get(8)?,
        last_seen_head: row.get(9)?,
        max_attempts: row.get::<_, i64>(10)? as u32,
        lease_owner: row.get(11)?,
        lease_acquired_at: row.get(12)?,
        lease_expires_at: row.get(13)?,
        heartbeat_at: row.get(14)?,
        lock_version: row.get::<_, i64>(15)? as u64,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

fn row_to_validation_command(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskValidationCommand> {
    Ok(TaskValidationCommand {
        task_id: parse_id(row.get(0)?, 0)?,
        position: row.get::<_, i64>(1)? as u32,
        command: row.get(2)?,
        created_at: row.get(3)?,
    })
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: parse_id(row.get(0)?, 0)?,
        task_id: parse_id(row.get(1)?, 1)?,
        parent_run_id: parse_optional_id(row.get(2)?, 2)?,
        status: parse_status(row.get::<_, String>(3)?, 3)?,
        repo_root: row.get(4)?,
        base_ref: row.get(5)?,
        base_commit: row.get(6)?,
        dirty_state_summary: row.get(7)?,
        current_phase: row.get(8)?,
        escalation_cycle: row.get::<_, i64>(9)? as u32,
        started_at: row.get(10)?,
        finished_at: row.get(11)?,
        final_diff_path: row.get(12)?,
        last_error: row.get(13)?,
    })
}

fn row_to_attempt(row: &rusqlite::Row<'_>) -> rusqlite::Result<Attempt> {
    Ok(Attempt {
        id: parse_id(row.get(0)?, 0)?,
        run_id: parse_id(row.get(1)?, 1)?,
        attempt_number: row.get::<_, i64>(2)? as u32,
        provider: row.get(3)?,
        model: row.get(4)?,
        status: parse_status(row.get::<_, String>(5)?, 5)?,
        prompt_path: row.get(6)?,
        response_path: row.get(7)?,
        patch_path: row.get(8)?,
        validation_log_path: row.get(9)?,
        validation_exit_code: row.get(10)?,
        failure_reason: row.get(11)?,
        apply_error: row.get(12)?,
        started_at: row.get(13)?,
        finished_at: row.get(14)?,
    })
}

fn row_to_ticket(row: &rusqlite::Row<'_>) -> rusqlite::Result<Ticket> {
    Ok(Ticket {
        id: parse_id(row.get(0)?, 0)?,
        task_id: parse_id(row.get(1)?, 1)?,
        run_id: parse_id(row.get(2)?, 2)?,
        status: parse_status(row.get::<_, String>(3)?, 3)?,
        blocked_on: row.get(4)?,
        question: row.get(5)?,
        reason: row.get(6)?,
        evidence_json: row.get(7)?,
        failure_fingerprint: row.get(8)?,
        created_at: row.get(9)?,
        resolved_at: row.get(10)?,
    })
}

fn row_to_ticket_resolution(row: &rusqlite::Row<'_>) -> rusqlite::Result<TicketResolution> {
    Ok(TicketResolution {
        id: parse_id(row.get(0)?, 0)?,
        ticket_id: parse_id(row.get(1)?, 1)?,
        provider: row.get(2)?,
        model: row.get(3)?,
        response_id: row.get(4)?,
        resolution_path: row.get(5)?,
        consumed_at: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn row_to_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        id: parse_id(row.get(0)?, 0)?,
        task_id: parse_id(row.get(1)?, 1)?,
        run_id: parse_optional_id(row.get(2)?, 2)?,
        attempt_id: parse_optional_id(row.get(3)?, 3)?,
        ticket_id: parse_optional_id(row.get(4)?, 4)?,
        kind: row.get(5)?,
        path: row.get(6)?,
        sha256: row.get(7)?,
        byte_len: row.get::<_, i64>(8)? as u64,
        created_at: row.get(9)?,
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    Ok(Event {
        id: parse_id(row.get(0)?, 0)?,
        task_id: parse_optional_id(row.get(1)?, 1)?,
        run_id: parse_optional_id(row.get(2)?, 2)?,
        kind: row.get(3)?,
        level: parse_status(row.get::<_, String>(4)?, 4)?,
        message: row.get(5)?,
        artifact_path: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn parse_id<T>(value: String, column: usize) -> rusqlite::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value
        .parse()
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(err)))
}

fn parse_optional_id<T>(value: Option<String>, column: usize) -> rusqlite::Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.map(|value| parse_id(value, column)).transpose()
}

fn parse_status<T>(value: String, column: usize) -> rusqlite::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value
        .parse()
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(err)))
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> HarnessResult<Vec<T>> {
    rows.collect::<Result<Vec<_>, _>>().map_err(sql_err)
}

fn apply_run_update(run: &mut Run, update: RunUpdate) {
    if let Some(repo_root) = update.repo_root {
        run.repo_root = repo_root;
    }
    if let Some(base_ref) = update.base_ref {
        run.base_ref = Some(base_ref);
    }
    if let Some(base_commit) = update.base_commit {
        run.base_commit = base_commit;
    }
    if let Some(dirty_state_summary) = update.dirty_state_summary {
        run.dirty_state_summary = Some(dirty_state_summary);
    }
    if let Some(current_phase) = update.current_phase {
        run.current_phase = Some(current_phase);
    }
    if let Some(finished_at) = update.finished_at {
        run.finished_at = Some(finished_at);
    }
    if let Some(final_diff_path) = update.final_diff_path {
        run.final_diff_path = Some(final_diff_path);
    }
    if let Some(last_error) = update.last_error {
        run.last_error = Some(last_error);
    }
}

fn valid_task_transition(from: TaskStatus, to: TaskStatus) -> bool {
    matches!(
        (from, to),
        (TaskStatus::Ready, TaskStatus::Running)
            | (TaskStatus::Running, TaskStatus::Complete)
            | (TaskStatus::Running, TaskStatus::Stuck)
            | (TaskStatus::Running, TaskStatus::Failed)
            | (TaskStatus::Stuck, TaskStatus::Running)
    )
}

fn valid_run_transition(from: RunStatus, to: RunStatus) -> bool {
    matches!(
        (from, to),
        (RunStatus::Running, RunStatus::Complete)
            | (RunStatus::Running, RunStatus::Stuck)
            | (RunStatus::Running, RunStatus::Failed)
    )
}

fn valid_ticket_transition(from: TicketStatus, to: TicketStatus) -> bool {
    matches!(
        (from, to),
        (TicketStatus::Open, TicketStatus::Resolving)
            | (TicketStatus::Resolving, TicketStatus::Resolved)
            | (TicketStatus::Resolving, TicketStatus::Failed)
            | (TicketStatus::Failed, TicketStatus::Resolving)
    )
}

fn assert_task_lease(conn: &Connection, task_id: &TaskId, owner: &str) -> HarnessResult<()> {
    let now = current_unix_secs();
    let held: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM tasks
                WHERE id = ?1
                  AND lease_owner = ?2
                  AND lease_expires_at IS NOT NULL
                  AND CAST(lease_expires_at AS INTEGER) > ?3
             )",
            params![task_id.as_str(), owner, now],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    if held {
        Ok(())
    } else {
        ensure_exists(conn, "tasks", task_id.as_str(), "task")?;
        Err(HarnessError::Conflict(format!(
            "task {} lease is not held by {owner} or has expired",
            task_id
        )))
    }
}

fn assert_parent_run_matches_task(conn: &Connection, run: &Run) -> HarnessResult<()> {
    let Some(parent_run_id) = &run.parent_run_id else {
        return Ok(());
    };
    let parent = get_run_with_conn(conn, parent_run_id)?;
    if parent.task_id == run.task_id {
        Ok(())
    } else {
        Err(HarnessError::Conflict(format!(
            "run {} task {} does not match parent run {} task {}",
            run.id, run.task_id, parent.id, parent.task_id
        )))
    }
}

fn assert_ticket_matches_run_and_lease(
    conn: &Connection,
    ticket: &Ticket,
    owner: &str,
) -> HarnessResult<()> {
    let run = get_run_with_conn(conn, &ticket.run_id)?;
    if run.task_id != ticket.task_id {
        return Err(HarnessError::Conflict(format!(
            "ticket {} task {} does not match run {} task {}",
            ticket.id, ticket.task_id, run.id, run.task_id
        )));
    }
    assert_task_lease(conn, &run.task_id, owner)
}

fn assert_artifact_matches_references_and_lease(
    conn: &Connection,
    artifact: &Artifact,
    owner: &str,
) -> HarnessResult<()> {
    if let Some(run_id) = &artifact.run_id {
        let run = get_run_with_conn(conn, run_id)?;
        if run.task_id != artifact.task_id {
            return Err(HarnessError::Conflict(format!(
                "artifact {} task {} does not match run {} task {}",
                artifact.id, artifact.task_id, run.id, run.task_id
            )));
        }
    }
    if let Some(attempt_id) = &artifact.attempt_id {
        let attempt = get_attempt_with_conn(conn, attempt_id)?;
        let attempt_run = get_run_with_conn(conn, &attempt.run_id)?;
        if attempt_run.task_id != artifact.task_id {
            return Err(HarnessError::Conflict(format!(
                "artifact {} task {} does not match attempt {} task {}",
                artifact.id, artifact.task_id, attempt.id, attempt_run.task_id
            )));
        }
        if let Some(run_id) = &artifact.run_id
            && &attempt.run_id != run_id
        {
            return Err(HarnessError::Conflict(format!(
                "artifact {} run {} does not match attempt {} run {}",
                artifact.id, run_id, attempt.id, attempt.run_id
            )));
        }
    }
    if let Some(ticket_id) = &artifact.ticket_id {
        let ticket = get_ticket_with_conn(conn, ticket_id)?;
        if ticket.task_id != artifact.task_id {
            return Err(HarnessError::Conflict(format!(
                "artifact {} task {} does not match ticket {} task {}",
                artifact.id, artifact.task_id, ticket.id, ticket.task_id
            )));
        }
    }
    assert_task_lease(conn, &artifact.task_id, owner)
}

fn assert_event_matches_references_and_lease(
    conn: &Connection,
    event: &Event,
    owner: &str,
) -> HarnessResult<()> {
    let task_id = match (&event.task_id, &event.run_id) {
        (Some(task_id), Some(run_id)) => {
            let run = get_run_with_conn(conn, run_id)?;
            if &run.task_id != task_id {
                return Err(HarnessError::Conflict(format!(
                    "event {} task {} does not match run {} task {}",
                    event.id, task_id, run.id, run.task_id
                )));
            }
            task_id.clone()
        }
        (Some(task_id), None) => task_id.clone(),
        (None, Some(run_id)) => get_run_with_conn(conn, run_id)?.task_id,
        (None, None) => return Ok(()),
    };
    assert_task_lease(conn, &task_id, owner)
}

fn has_ticket_resolution(conn: &Connection, ticket_id: &TicketId) -> HarnessResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM ticket_resolutions WHERE ticket_id = ?1)",
        params![ticket_id.as_str()],
        |row| row.get(0),
    )
    .map_err(sql_err)
}

fn has_resolved_unconsumed_resolution_for_task(
    conn: &Connection,
    task_id: &TaskId,
) -> HarnessResult<bool> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM ticket_resolutions tr
            JOIN tickets t ON t.id = tr.ticket_id
            WHERE t.task_id = ?1
              AND t.status = 'resolved'
              AND tr.consumed_at IS NULL
        )",
        params![task_id.as_str()],
        |row| row.get(0),
    )
    .map_err(sql_err)
}

fn ensure_exists(
    conn: &Connection,
    table: &'static str,
    id: &str,
    kind: &'static str,
) -> HarnessResult<()> {
    let exists: bool = conn
        .query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = ?1)"),
            params![id],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    if exists {
        Ok(())
    } else {
        Err(not_found(kind, id))
    }
}

fn sql_err(err: rusqlite::Error) -> HarnessError {
    HarnessError::External(format!("sqlite: {err}"))
}

fn not_found(kind: &'static str, id: &str) -> HarnessError {
    HarnessError::NotFound {
        kind,
        id: id.to_string(),
    }
}

fn current_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn now_string() -> String {
    current_unix_secs().to_string()
}

fn checksum(input: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ArtifactId, AttemptStatus, EventId, EventLevel};

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_ID: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const ATTEMPT_ID: &str = "att_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_ID: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const OTHER_TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const OTHER_RUN_ID: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const RES_ID: &str = "res_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const ART_ID: &str = "art_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const EVENT_ID: &str = "event_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const OWNER: &str = "test-owner";

    #[test]
    fn state_store_migrates_fresh_database_and_sets_pragmas() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.sqlite");
        let store = SqliteTaskStore::open(&db_path).unwrap();

        assert_eq!(store.pragma_value("foreign_keys").unwrap(), "1");
        assert_eq!(store.pragma_value("busy_timeout").unwrap(), "5000");
        assert_eq!(store.pragma_value("journal_mode").unwrap(), "wal");

        let conn = store.lock_conn().unwrap();
        let migration_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        let task_columns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migration_count, 1);
        assert_eq!(task_columns, 18);
    }

    #[test]
    fn state_store_round_trips_core_repositories() {
        let store = SqliteTaskStore::in_memory().unwrap();
        let task = sample_task(TaskStatus::Ready);
        store
            .insert_task(
                task.clone(),
                vec!["cargo test".to_string(), "cargo fmt --check".to_string()],
            )
            .unwrap();

        assert_eq!(store.get_task(&task.id).unwrap(), task);
        assert_eq!(store.list_tasks(Some(TaskStatus::Ready)).unwrap().len(), 1);
        let validation = store.list_validation_commands(&task.id).unwrap();
        assert_eq!(validation[0].position, 0);
        assert_eq!(validation[1].command, "cargo fmt --check");
        store.acquire_task_lease(&task.id, OWNER).unwrap();

        let run = sample_run(RunStatus::Running);
        store.insert_run(run.clone(), OWNER).unwrap();
        assert_eq!(store.get_run(&run.id).unwrap(), run);
        assert_eq!(
            store.latest_run_for_task(&task.id).unwrap().unwrap().id,
            run.id
        );

        let attempt = sample_attempt(AttemptStatus::ValidationFailed);
        store.insert_attempt(attempt.clone(), OWNER).unwrap();
        assert_eq!(store.list_attempts(&run.id).unwrap(), vec![attempt.clone()]);

        let ticket = sample_ticket(TicketStatus::Open);
        store.insert_ticket(ticket.clone(), OWNER).unwrap();
        assert_eq!(store.get_ticket(&ticket.id).unwrap(), ticket);
        assert_eq!(
            store
                .list_tickets(Some(&task.id), Some(TicketStatus::Open))
                .unwrap()
                .len(),
            1
        );

        store
            .transition_ticket(
                &ticket.id,
                TicketStatus::Open,
                TicketStatus::Resolving,
                OWNER,
            )
            .unwrap();
        let resolution = sample_resolution();
        store
            .insert_ticket_resolution(resolution.clone(), OWNER)
            .unwrap();
        assert_eq!(
            store.get_ticket_resolution(&resolution.id).unwrap(),
            resolution
        );
        assert_eq!(
            store
                .latest_unconsumed_resolution_for_ticket(&ticket.id)
                .unwrap(),
            Some(resolution.clone())
        );
        assert_eq!(
            store.latest_unconsumed_resolution_for_run(&run.id).unwrap(),
            Some(resolution.clone())
        );
        assert_eq!(
            store.latest_unconsumed_resolution(&task.id).unwrap(),
            Some(resolution.clone())
        );
        store
            .mark_ticket_resolution_consumed(&resolution.id, "2026-01-01T00:00:10Z", OWNER)
            .unwrap();
        assert!(
            store
                .latest_unconsumed_resolution_for_ticket(&ticket.id)
                .unwrap()
                .is_none()
        );

        let artifact = sample_artifact();
        store.insert_artifact(artifact.clone(), OWNER).unwrap();
        assert_eq!(
            store.list_artifacts_for_run(&run.id).unwrap(),
            vec![artifact]
        );

        let event = sample_event();
        store.insert_event(event.clone(), OWNER).unwrap();
        assert_eq!(store.list_events_for_task(&task.id).unwrap(), vec![event]);
    }

    #[test]
    fn state_store_enforces_status_transitions() {
        let store = SqliteTaskStore::in_memory().unwrap();
        let task = sample_task(TaskStatus::Ready);
        store
            .insert_task(task.clone(), vec!["cargo test".to_string()])
            .unwrap();
        store.acquire_task_lease(&task.id, OWNER).unwrap();
        let running = store
            .transition_task(&task.id, TaskStatus::Ready, TaskStatus::Running, OWNER)
            .unwrap();
        assert_eq!(running.status, TaskStatus::Running);
        assert!(
            store
                .transition_task(&task.id, TaskStatus::Complete, TaskStatus::Running, OWNER)
                .is_err()
        );
        assert!(
            store
                .transition_task(&task.id, TaskStatus::Running, TaskStatus::Ready, OWNER)
                .is_err()
        );

        let run = sample_run(RunStatus::Running);
        store.insert_run(run.clone(), OWNER).unwrap();
        let updated = store
            .update_run(
                &run.id,
                Some(RunStatus::Running),
                RunUpdate {
                    status: Some(RunStatus::Stuck),
                    last_error: Some("needs help".to_string()),
                    ..RunUpdate::default()
                },
                OWNER,
            )
            .unwrap();
        assert_eq!(updated.status, RunStatus::Stuck);
        assert!(
            store
                .update_run(
                    &run.id,
                    Some(RunStatus::Stuck),
                    RunUpdate {
                        status: Some(RunStatus::Complete),
                        ..RunUpdate::default()
                    },
                    OWNER,
                )
                .is_err()
        );

        let ticket = sample_ticket(TicketStatus::Open);
        store.insert_ticket(ticket.clone(), OWNER).unwrap();
        assert_eq!(
            store
                .transition_ticket(
                    &ticket.id,
                    TicketStatus::Open,
                    TicketStatus::Resolving,
                    OWNER
                )
                .unwrap()
                .status,
            TicketStatus::Resolving
        );
        assert!(
            store
                .transition_ticket(
                    &ticket.id,
                    TicketStatus::Resolved,
                    TicketStatus::Open,
                    OWNER
                )
                .is_err()
        );
    }

    #[test]
    fn state_store_acquires_conflicts_reclaims_heartbeats_and_recovers_leases() {
        let store = SqliteTaskStore::in_memory()
            .unwrap()
            .with_lease_ttl_secs(300);
        let mut task = sample_task(TaskStatus::Running);
        task.lease_owner = Some("old-owner".to_string());
        task.lease_acquired_at = Some("100".to_string());
        task.lease_expires_at = Some("200".to_string());
        task.heartbeat_at = Some("100".to_string());
        store
            .insert_task(task.clone(), vec!["cargo test".to_string()])
            .unwrap();
        store.acquire_task_lease(&task.id, OWNER).unwrap();
        let run = sample_run(RunStatus::Running);
        store.insert_run(run.clone(), OWNER).unwrap();
        store.release_task_lease(&task.id, OWNER).unwrap();

        assert!(
            store
                .acquire_task_lease(&task.id, "new-owner")
                .unwrap()
                .owner
                == "new-owner"
        );
        assert!(store.acquire_task_lease(&task.id, "other-owner").is_err());
        store.heartbeat_task_lease(&task.id, "new-owner").unwrap();
        assert!(store.heartbeat_task_lease(&task.id, "other-owner").is_err());
        store.release_task_lease(&task.id, "new-owner").unwrap();
        assert!(
            store
                .acquire_task_lease(&task.id, "owner-after-release")
                .is_ok()
        );

        {
            let conn = store.lock_conn().unwrap();
            conn.execute(
                "UPDATE tasks SET lease_owner = 'expired-owner', lease_expires_at = '1', status = 'running' WHERE id = ?1",
                params![task.id.as_str()],
            )
            .unwrap();
        }
        let recovered = store.recover_expired_leases("2").unwrap();
        assert_eq!(recovered, vec![task.id.clone()]);
        let recovered_task = store.get_task(&task.id).unwrap();
        let recovered_run = store.get_run(&run.id).unwrap();
        assert_eq!(recovered_task.status, TaskStatus::Failed);
        assert_eq!(recovered_task.lease_owner, None);
        assert_eq!(recovered_run.status, RunStatus::Failed);
        assert_eq!(recovered_run.last_error.as_deref(), Some("lease expired"));
    }

    #[test]
    fn state_store_create_or_get_ticket_is_idempotent_by_fingerprint() {
        let store = SqliteTaskStore::in_memory().unwrap();
        let task = sample_task(TaskStatus::Running);
        store
            .insert_task(task.clone(), vec!["cargo test".to_string()])
            .unwrap();
        store.acquire_task_lease(&task.id, OWNER).unwrap();
        store
            .insert_run(sample_run(RunStatus::Running), OWNER)
            .unwrap();

        let first = sample_ticket(TicketStatus::Open);
        let mut second = first.clone();
        second.id = TicketId::parse("ticket_01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap();
        second.question = "different phrasing".to_string();

        assert_eq!(
            store.create_or_get_ticket(first.clone(), OWNER).unwrap(),
            first
        );
        assert_eq!(store.create_or_get_ticket(second, OWNER).unwrap(), first);

        let mut other_blocker = first.clone();
        other_blocker.id = TicketId::parse("ticket_01ARZ3NDEKTSV4RRFFQ69G5FAX").unwrap();
        other_blocker.blocked_on = "provider_limit".to_string();
        assert_eq!(
            store
                .create_or_get_ticket(other_blocker.clone(), OWNER)
                .unwrap(),
            other_blocker
        );
    }

    #[test]
    fn state_store_rejects_task_mutations_without_current_lease() {
        let store = SqliteTaskStore::in_memory().unwrap();
        let task = sample_task(TaskStatus::Running);
        store
            .insert_task(task.clone(), vec!["cargo test".to_string()])
            .unwrap();

        assert!(
            store
                .insert_run(sample_run(RunStatus::Running), OWNER)
                .is_err()
        );

        store.acquire_task_lease(&task.id, OWNER).unwrap();
        store
            .insert_run(sample_run(RunStatus::Running), OWNER)
            .unwrap();
        store.release_task_lease(&task.id, OWNER).unwrap();

        assert!(
            store
                .update_run(
                    &RunId::parse(RUN_ID).unwrap(),
                    Some(RunStatus::Running),
                    RunUpdate {
                        status: Some(RunStatus::Stuck),
                        ..RunUpdate::default()
                    },
                    OWNER,
                )
                .is_err()
        );
        assert!(
            store
                .create_or_get_ticket(sample_ticket(TicketStatus::Open), OWNER)
                .is_err()
        );
    }

    #[test]
    fn state_store_update_task_preserves_lease_fields() {
        let store = SqliteTaskStore::in_memory().unwrap();
        let task = sample_task(TaskStatus::Running);
        store
            .insert_task(task.clone(), vec!["cargo test".to_string()])
            .unwrap();
        store.acquire_task_lease(&task.id, OWNER).unwrap();

        let mut update = store.get_task(&task.id).unwrap();
        update.lease_owner = Some("wrong-owner".to_string());
        update.lease_acquired_at = None;
        update.lease_expires_at = None;
        update.heartbeat_at = None;
        update.status = TaskStatus::Complete;
        update.worktree_path = Some("/tmp/worktree".to_string());
        store.update_task(update, OWNER).unwrap();

        let updated = store.get_task(&task.id).unwrap();
        assert_eq!(updated.worktree_path.as_deref(), Some("/tmp/worktree"));
        assert_eq!(updated.status, TaskStatus::Running);
        assert_eq!(updated.lease_owner.as_deref(), Some(OWNER));
        assert!(updated.lease_expires_at.is_some());
    }

    #[test]
    fn state_store_rejects_cross_task_run_scoped_records() {
        let store = SqliteTaskStore::in_memory().unwrap();
        let task = sample_task(TaskStatus::Running);
        let other_task = sample_other_task(TaskStatus::Running);
        store
            .insert_task(task.clone(), vec!["cargo test".to_string()])
            .unwrap();
        store
            .insert_task(other_task.clone(), vec!["cargo test".to_string()])
            .unwrap();
        store.acquire_task_lease(&task.id, OWNER).unwrap();
        store
            .acquire_task_lease(&other_task.id, "other-owner")
            .unwrap();
        store
            .insert_run(sample_run(RunStatus::Running), OWNER)
            .unwrap();
        store
            .insert_run(sample_other_run(RunStatus::Running), "other-owner")
            .unwrap();
        let mut bad_child_run = sample_run(RunStatus::Running);
        bad_child_run.id = RunId::parse("run_01ARZ3NDEKTSV4RRFFQ69G5FAX").unwrap();
        bad_child_run.parent_run_id = Some(RunId::parse(OTHER_RUN_ID).unwrap());
        assert!(store.insert_run(bad_child_run, OWNER).is_err());
        store
            .insert_attempt(sample_attempt(AttemptStatus::ValidationFailed), OWNER)
            .unwrap();
        let mut other_attempt = sample_attempt(AttemptStatus::ValidationFailed);
        other_attempt.id = AttemptId::parse("att_01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap();
        other_attempt.run_id = RunId::parse(OTHER_RUN_ID).unwrap();
        store.insert_attempt(other_attempt, "other-owner").unwrap();

        let mut bad_ticket = sample_ticket(TicketStatus::Open);
        bad_ticket.run_id = RunId::parse(OTHER_RUN_ID).unwrap();
        assert!(store.insert_ticket(bad_ticket, OWNER).is_err());

        let mut bad_artifact = sample_artifact();
        bad_artifact.run_id = Some(RunId::parse(OTHER_RUN_ID).unwrap());
        assert!(store.insert_artifact(bad_artifact, OWNER).is_err());
        let mut bad_attempt_artifact = sample_artifact();
        bad_attempt_artifact.attempt_id =
            Some(AttemptId::parse("att_01ARZ3NDEKTSV4RRFFQ69G5FAW").unwrap());
        assert!(store.insert_artifact(bad_attempt_artifact, OWNER).is_err());

        let mut bad_event = sample_event();
        bad_event.run_id = Some(RunId::parse(OTHER_RUN_ID).unwrap());
        assert!(store.insert_event(bad_event, OWNER).is_err());
    }

    #[test]
    fn state_store_resume_requires_resolved_unconsumed_ticket_resolution() {
        let store = SqliteTaskStore::in_memory().unwrap();
        let task = sample_task(TaskStatus::Stuck);
        store
            .insert_task(task.clone(), vec!["cargo test".to_string()])
            .unwrap();
        store.acquire_task_lease(&task.id, OWNER).unwrap();
        store
            .insert_run(sample_run(RunStatus::Stuck), OWNER)
            .unwrap();
        let ticket = sample_ticket(TicketStatus::Open);
        store.insert_ticket(ticket.clone(), OWNER).unwrap();

        assert!(
            store
                .transition_task(&task.id, TaskStatus::Stuck, TaskStatus::Running, OWNER)
                .is_err()
        );
        assert!(
            store
                .latest_unconsumed_resolution_for_ticket(&ticket.id)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .transition_ticket(
                    &ticket.id,
                    TicketStatus::Open,
                    TicketStatus::Resolved,
                    OWNER
                )
                .is_err()
        );

        store
            .transition_ticket(
                &ticket.id,
                TicketStatus::Open,
                TicketStatus::Resolving,
                OWNER,
            )
            .unwrap();
        let resolution = sample_resolution();
        store
            .insert_ticket_resolution(resolution.clone(), OWNER)
            .unwrap();
        assert_eq!(
            store.get_ticket(&ticket.id).unwrap().status,
            TicketStatus::Resolved
        );
        assert_eq!(
            store
                .latest_unconsumed_resolution_for_ticket(&ticket.id)
                .unwrap(),
            Some(resolution)
        );

        let running = store
            .transition_task(&task.id, TaskStatus::Stuck, TaskStatus::Running, OWNER)
            .unwrap();
        assert_eq!(running.status, TaskStatus::Running);
    }

    fn sample_task(status: TaskStatus) -> Task {
        Task {
            id: TaskId::parse(TASK_ID).unwrap(),
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

    fn sample_other_task(status: TaskStatus) -> Task {
        let mut task = sample_task(status);
        task.id = TaskId::parse(OTHER_TASK_ID).unwrap();
        task.title = "Other task".to_string();
        task
    }

    fn sample_run(status: RunStatus) -> Run {
        Run {
            id: RunId::parse(RUN_ID).unwrap(),
            task_id: TaskId::parse(TASK_ID).unwrap(),
            parent_run_id: None,
            status,
            repo_root: "/repo".to_string(),
            base_ref: Some("main".to_string()),
            base_commit: "abcdef".to_string(),
            dirty_state_summary: None,
            current_phase: Some("validate".to_string()),
            escalation_cycle: 0,
            started_at: "2026-01-01T00:00:01Z".to_string(),
            finished_at: None,
            final_diff_path: None,
            last_error: None,
        }
    }

    fn sample_other_run(status: RunStatus) -> Run {
        let mut run = sample_run(status);
        run.id = RunId::parse(OTHER_RUN_ID).unwrap();
        run.task_id = TaskId::parse(OTHER_TASK_ID).unwrap();
        run
    }

    fn sample_attempt(status: AttemptStatus) -> Attempt {
        Attempt {
            id: AttemptId::parse(ATTEMPT_ID).unwrap(),
            run_id: RunId::parse(RUN_ID).unwrap(),
            attempt_number: 1,
            provider: "fake".to_string(),
            model: "model".to_string(),
            status,
            prompt_path: Some("prompt.md".to_string()),
            response_path: Some("response.md".to_string()),
            patch_path: Some("patch.diff".to_string()),
            validation_log_path: Some("validation.log".to_string()),
            validation_exit_code: Some(101),
            failure_reason: Some("tests failed".to_string()),
            apply_error: None,
            started_at: "2026-01-01T00:00:02Z".to_string(),
            finished_at: Some("2026-01-01T00:00:03Z".to_string()),
        }
    }

    fn sample_ticket(status: TicketStatus) -> Ticket {
        Ticket {
            id: TicketId::parse(TICKET_ID).unwrap(),
            task_id: TaskId::parse(TASK_ID).unwrap(),
            run_id: RunId::parse(RUN_ID).unwrap(),
            status,
            blocked_on: "validation".to_string(),
            question: "How should this test be fixed?".to_string(),
            reason: "Repeated failure".to_string(),
            evidence_json: r#"{"log":"redacted"}"#.to_string(),
            failure_fingerprint: "fingerprint-1".to_string(),
            created_at: "2026-01-01T00:00:04Z".to_string(),
            resolved_at: None,
        }
    }

    fn sample_resolution() -> TicketResolution {
        TicketResolution {
            id: TicketResolutionId::parse(RES_ID).unwrap(),
            ticket_id: TicketId::parse(TICKET_ID).unwrap(),
            provider: "fake-openai".to_string(),
            model: "gpt-test".to_string(),
            response_id: Some("resp_1".to_string()),
            resolution_path: "resolution.md".to_string(),
            consumed_at: None,
            created_at: "2026-01-01T00:00:05Z".to_string(),
        }
    }

    fn sample_artifact() -> Artifact {
        Artifact {
            id: ArtifactId::parse(ART_ID).unwrap(),
            task_id: TaskId::parse(TASK_ID).unwrap(),
            run_id: Some(RunId::parse(RUN_ID).unwrap()),
            attempt_id: Some(AttemptId::parse(ATTEMPT_ID).unwrap()),
            ticket_id: Some(TicketId::parse(TICKET_ID).unwrap()),
            kind: "validation_log".to_string(),
            path: "validation.log".to_string(),
            sha256: "00".repeat(32),
            byte_len: 42,
            created_at: "2026-01-01T00:00:06Z".to_string(),
        }
    }

    fn sample_event() -> Event {
        Event {
            id: EventId::parse(EVENT_ID).unwrap(),
            task_id: Some(TaskId::parse(TASK_ID).unwrap()),
            run_id: Some(RunId::parse(RUN_ID).unwrap()),
            kind: "attempt.finished".to_string(),
            level: EventLevel::Info,
            message: "attempt finished".to_string(),
            artifact_path: Some("validation.log".to_string()),
            created_at: "2026-01-01T00:00:07Z".to_string(),
        }
    }
}
