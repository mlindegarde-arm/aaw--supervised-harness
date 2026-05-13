use crate::domain::{
    ArtifactId, AttemptId, AttemptStatus, EventId, EventLevel, RunId, RunStatus, TaskId,
    TaskStatus, TicketId, TicketResolutionId, TicketStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub goal: String,
    pub status: TaskStatus,
    pub repo_root: String,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub base_ref: Option<String>,
    pub base_commit: Option<String>,
    pub last_seen_head: Option<String>,
    pub max_attempts: u32,
    pub lease_owner: Option<String>,
    pub lease_acquired_at: Option<String>,
    pub lease_expires_at: Option<String>,
    pub heartbeat_at: Option<String>,
    pub lock_version: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskValidationCommand {
    pub task_id: TaskId,
    pub position: u32,
    pub command: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub id: RunId,
    pub task_id: TaskId,
    pub parent_run_id: Option<RunId>,
    pub status: RunStatus,
    pub repo_root: String,
    pub base_ref: Option<String>,
    pub base_commit: String,
    pub dirty_state_summary: Option<String>,
    pub current_phase: Option<String>,
    pub escalation_cycle: u32,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub final_diff_path: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub id: AttemptId,
    pub run_id: RunId,
    pub attempt_number: u32,
    pub provider: String,
    pub model: String,
    pub status: AttemptStatus,
    pub prompt_path: Option<String>,
    pub response_path: Option<String>,
    pub patch_path: Option<String>,
    pub validation_log_path: Option<String>,
    pub validation_exit_code: Option<i32>,
    pub failure_reason: Option<String>,
    pub apply_error: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub id: TicketId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub status: TicketStatus,
    pub blocked_on: String,
    pub question: String,
    pub reason: String,
    pub evidence_json: String,
    pub failure_fingerprint: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketResolution {
    pub id: TicketResolutionId,
    pub ticket_id: TicketId,
    pub provider: String,
    pub model: String,
    pub response_id: Option<String>,
    pub resolution_path: String,
    pub consumed_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub id: ArtifactId,
    pub task_id: TaskId,
    pub run_id: Option<RunId>,
    pub attempt_id: Option<AttemptId>,
    pub ticket_id: Option<TicketId>,
    pub kind: String,
    pub path: String,
    pub sha256: String,
    pub byte_len: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: EventId,
    pub task_id: Option<TaskId>,
    pub run_id: Option<RunId>,
    pub kind: String,
    pub level: EventLevel,
    pub message: String,
    pub artifact_path: Option<String>,
    pub created_at: String,
}
