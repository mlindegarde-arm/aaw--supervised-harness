use crate::HarnessError;
use crate::HarnessResult;
use crate::domain::{RunId, TaskId, TaskStatus, TicketId, TicketResolution};
use crate::orchestrator::RunOrchestrator;
use crate::orchestrator::supervisor_state::{
    SupervisorTicketSelection, select_ticket_for_latest_stuck_run,
};
use crate::runtime::{
    CancellationToken, CommandEvent, CommandEventLevel, CommandExit, CommandResult, CommandStatus,
    OutputSink, ResumeTaskOptions, SuperviseCreateOptions, SuperviseProgressEvent,
    SuperviseProgressPhase, SuperviseTaskOptions, TaskRunOptions, TicketResolveOptions,
};
use serde_json::{Value, json};

impl RunOrchestrator {
    pub fn supervise_task(
        &self,
        task_id: &TaskId,
        options: SuperviseTaskOptions,
    ) -> HarnessResult<CommandResult> {
        self.supervise_task_with_cancellation(task_id, options, &NeverCancelled)
    }

    pub fn create_and_supervise_task(
        &self,
        options: SuperviseCreateOptions,
    ) -> HarnessResult<CommandResult> {
        let task = self.create_task(
            options.title.clone(),
            options.goal.clone(),
            options.validation_commands.clone(),
        )?;
        let supervise = SuperviseTaskOptions {
            runtime: options.runtime,
            ticket_id: None,
            max_attempts: options.max_attempts,
            model: options.model,
            ticket_model: options.ticket_model,
            max_cycles: options.max_cycles,
        };
        self.supervise_task(&task.id, supervise)
    }

    pub fn supervise_task_with_cancellation(
        &self,
        task_id: &TaskId,
        options: SuperviseTaskOptions,
        cancellation: &dyn CancellationToken,
    ) -> HarnessResult<CommandResult> {
        self.supervise_task_with_cancellation_and_progress_sink(
            task_id,
            options,
            cancellation,
            None,
        )
    }

    pub fn supervise_task_with_progress_sink(
        &self,
        task_id: &TaskId,
        options: SuperviseTaskOptions,
        progress_sink: &mut dyn OutputSink,
    ) -> HarnessResult<CommandResult> {
        self.supervise_task_with_cancellation_and_progress_sink(
            task_id,
            options,
            &NeverCancelled,
            Some(progress_sink),
        )
    }

    pub fn create_and_supervise_task_with_progress_sink(
        &self,
        options: SuperviseCreateOptions,
        progress_sink: &mut dyn OutputSink,
    ) -> HarnessResult<CommandResult> {
        let task = self.create_task(
            options.title.clone(),
            options.goal.clone(),
            options.validation_commands.clone(),
        )?;
        let supervise = SuperviseTaskOptions {
            runtime: options.runtime,
            ticket_id: None,
            max_attempts: options.max_attempts,
            model: options.model,
            ticket_model: options.ticket_model,
            max_cycles: options.max_cycles,
        };
        self.supervise_task_with_progress_sink(&task.id, supervise, progress_sink)
    }

    fn supervise_task_with_cancellation_and_progress_sink(
        &self,
        task_id: &TaskId,
        options: SuperviseTaskOptions,
        cancellation: &dyn CancellationToken,
        progress_sink: Option<&mut dyn OutputSink>,
    ) -> HarnessResult<CommandResult> {
        let cap = options
            .max_cycles
            .unwrap_or(self.config.orchestrator.max_escalation_cycles)
            .min(self.config.orchestrator.max_escalation_cycles);
        let mut aggregation = SupervisorAggregation::new(task_id.clone(), cap, progress_sink);
        let mut requested_ticket = options.ticket_id.clone();

        loop {
            if cancellation.is_cancelled() {
                return aggregation.cancelled(task_id);
            }

            self.store.recover_expired_supervisor_leases(task_id)?;
            let snapshot = self.store.supervisor_task_state(task_id)?;
            aggregation.push(progress(
                SuperviseProgressPhase::InspectTask,
                task_id,
                None,
                requested_ticket.as_ref(),
                None,
                format!("inspecting task {}", task_id),
                None,
            ))?;

            match snapshot.task.status {
                TaskStatus::Complete => {
                    return aggregation.complete(task_id);
                }
                TaskStatus::Failed => {
                    return aggregation.failed(
                        CommandExit::failure(format!("task {} is failed", task_id)),
                        next_for_task(task_id),
                    );
                }
                TaskStatus::Running => {
                    return aggregation.failed(
                        CommandExit::failure(format!("task {} is already running", task_id)),
                        next_for_task(task_id),
                    );
                }
                TaskStatus::Ready => {
                    if cancellation.is_cancelled() {
                        return aggregation.cancelled(task_id);
                    }
                    aggregation.push(progress(
                        SuperviseProgressPhase::RunTask,
                        task_id,
                        None,
                        None,
                        Some(0),
                        format!("running task {}", task_id),
                        Some(format!("harness task get {} --output json", task_id)),
                    ))?;
                    let result = self.run_task(
                        task_id,
                        TaskRunOptions {
                            runtime: options.runtime.clone(),
                            max_attempts: options.max_attempts,
                            model: options.model.clone(),
                        },
                    );
                    let result = match classify_result(result, task_id, None) {
                        Ok(result) => result,
                        Err(exit) => return aggregation.failed(exit, next_for_task(task_id)),
                    };
                    aggregation.merge(&result);
                    match result.exit.status {
                        CommandStatus::Complete => return aggregation.complete(task_id),
                        CommandStatus::Stuck => {
                            requested_ticket = ticket_from_data(&result.data).or(requested_ticket);
                        }
                        _ => return aggregation.finish_with(result),
                    }
                }
                TaskStatus::Stuck => {
                    let selection = match select_ticket_for_latest_stuck_run(
                        self.store.as_ref(),
                        task_id,
                        requested_ticket.as_ref(),
                    ) {
                        Ok(Some(selection)) => selection,
                        Ok(None) => {
                            return aggregation.stuck(
                                task_id,
                                None,
                                format!("task {} is stuck but has no retryable ticket", task_id),
                            );
                        }
                        Err(err) => {
                            return aggregation.stuck(
                                task_id,
                                requested_ticket.as_ref(),
                                err.to_string(),
                            );
                        }
                    };

                    let ticket_id = selection.ticket.ticket().id.clone();
                    let run_id = selection.stuck_run.run.id.clone();
                    let next_cycle = selection.stuck_run.next_cycle;
                    aggregation.cycles = aggregation.cycles.max(next_cycle.saturating_sub(1));

                    if next_cycle > cap {
                        return aggregation.stuck(
                            task_id,
                            Some(&ticket_id),
                            format!(
                                "task {} remains stuck after {} supervised cycles",
                                task_id, cap
                            ),
                        );
                    }

                    if let SupervisorTicketSelection::ResolvedUnconsumed { ticket, resolution } =
                        &selection.ticket
                    {
                        aggregation.record_ticket_resolution(&ticket.id, resolution);
                    } else {
                        if cancellation.is_cancelled() {
                            return aggregation.cancelled(task_id);
                        }
                        aggregation.push(progress(
                            SuperviseProgressPhase::ResolveTicket,
                            task_id,
                            Some(&run_id),
                            Some(&ticket_id),
                            Some(next_cycle),
                            format!("resolving ticket {}", ticket_id),
                            Some(format!("harness ticket get {} --output json", ticket_id)),
                        ))?;
                        let resolved = self.resolve_ticket(
                            &ticket_id,
                            TicketResolveOptions {
                                runtime: options.runtime.clone(),
                                model: options.ticket_model.clone(),
                            },
                        );
                        let resolved = match classify_result(resolved, task_id, Some(&ticket_id)) {
                            Ok(result) => result,
                            Err(exit) => {
                                return aggregation
                                    .failed(exit, next_for_ticket(task_id, &ticket_id));
                            }
                        };
                        aggregation.merge(&resolved);
                        if resolved.exit.status != CommandStatus::Complete {
                            return aggregation.finish_with(resolved);
                        }
                        aggregation.record_resolution_data(&resolved.data);
                    }

                    let selection = match select_ticket_for_latest_stuck_run(
                        self.store.as_ref(),
                        task_id,
                        Some(&ticket_id),
                    ) {
                        Ok(Some(selection)) => selection,
                        Ok(None) => {
                            return aggregation.stuck(
                                task_id,
                                Some(&ticket_id),
                                format!("ticket {} has no unconsumed resolution", ticket_id),
                            );
                        }
                        Err(err) => {
                            return aggregation.failed(
                                classify_error(err, Some(task_id), Some(&ticket_id)),
                                next_for_ticket(task_id, &ticket_id),
                            );
                        }
                    };

                    if !matches!(
                        selection.ticket,
                        SupervisorTicketSelection::ResolvedUnconsumed { .. }
                    ) {
                        return aggregation.stuck(
                            task_id,
                            Some(&ticket_id),
                            format!(
                                "ticket {} did not produce an unconsumed resolution",
                                ticket_id
                            ),
                        );
                    }

                    if cancellation.is_cancelled() {
                        return aggregation.cancelled(task_id);
                    }
                    self.store
                        .mark_supervisor_cycle_started(task_id, next_cycle)?;
                    aggregation.cycles = aggregation.cycles.max(next_cycle);
                    aggregation.push(progress(
                        SuperviseProgressPhase::ResumeTask,
                        task_id,
                        Some(&run_id),
                        Some(&ticket_id),
                        Some(next_cycle),
                        format!("resuming task {} with ticket {}", task_id, ticket_id),
                        Some(format!(
                            "harness resume {} --ticket {} --output json",
                            task_id, ticket_id
                        )),
                    ))?;
                    let resumed = self.resume_task(
                        task_id,
                        ResumeTaskOptions {
                            runtime: options.runtime.clone(),
                            ticket_id: Some(ticket_id.clone()),
                            max_attempts: options.max_attempts,
                            model: options.model.clone(),
                        },
                    );
                    let resumed = match classify_result(resumed, task_id, Some(&ticket_id)) {
                        Ok(result) => result,
                        Err(exit) => {
                            return aggregation.failed(exit, next_for_ticket(task_id, &ticket_id));
                        }
                    };
                    aggregation.merge(&resumed);
                    match resumed.exit.status {
                        CommandStatus::Complete => return aggregation.complete(task_id),
                        CommandStatus::Stuck => {
                            requested_ticket = ticket_from_data(&resumed.data);
                        }
                        _ => return aggregation.finish_with(resumed),
                    }
                }
            }
        }
    }
}

struct NeverCancelled;

impl CancellationToken for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

struct SupervisorAggregation<'a> {
    task_id: TaskId,
    max_cycles: u32,
    cycles: u32,
    events: Vec<CommandEvent>,
    progress_events: Vec<SuperviseProgressEvent>,
    run_ids: Vec<String>,
    resolved_tickets: Vec<String>,
    resolution_ids: Vec<String>,
    progress_sink: Option<&'a mut dyn OutputSink>,
}

impl<'a> SupervisorAggregation<'a> {
    fn new(
        task_id: TaskId,
        max_cycles: u32,
        progress_sink: Option<&'a mut dyn OutputSink>,
    ) -> Self {
        Self {
            task_id,
            max_cycles,
            cycles: 0,
            events: Vec::new(),
            progress_events: Vec::new(),
            run_ids: Vec::new(),
            resolved_tickets: Vec::new(),
            resolution_ids: Vec::new(),
            progress_sink,
        }
    }

    fn push(&mut self, event: SuperviseProgressEvent) -> HarnessResult<()> {
        let command_event = command_event(&event);
        if let Some(sink) = &mut self.progress_sink {
            sink.event(&command_event)?;
        }
        self.events.push(command_event);
        self.progress_events.push(event);
        Ok(())
    }

    fn merge(&mut self, result: &CommandResult) {
        if let Some(run_id) = result.data.get("run_id").and_then(Value::as_str) {
            push_unique(&mut self.run_ids, run_id);
        }
    }

    fn record_resolution_data(&mut self, data: &Value) {
        if let Some(ticket_id) = data.get("ticket_id").and_then(Value::as_str) {
            push_unique(&mut self.resolved_tickets, ticket_id);
        }
        if let Some(resolution_id) = data.get("resolution_id").and_then(Value::as_str) {
            push_unique(&mut self.resolution_ids, resolution_id);
        }
    }

    fn record_ticket_resolution(&mut self, ticket_id: &TicketId, resolution: &TicketResolution) {
        push_unique(&mut self.resolved_tickets, ticket_id.as_str());
        push_unique(&mut self.resolution_ids, resolution.id.as_str());
    }

    fn complete(mut self, task_id: &TaskId) -> HarnessResult<CommandResult> {
        self.push(progress(
            SuperviseProgressPhase::Complete,
            task_id,
            None,
            None,
            Some(self.cycles),
            format!("task {} complete after supervision", task_id),
            None,
        ))?;
        Ok(self.result(
            CommandExit::new(
                CommandStatus::Complete,
                0,
                Some(format!("task {} complete after supervision", task_id)),
            ),
            Vec::new(),
        ))
    }

    fn stuck(
        mut self,
        task_id: &TaskId,
        ticket_id: Option<&TicketId>,
        message: String,
    ) -> HarnessResult<CommandResult> {
        self.push(progress(
            SuperviseProgressPhase::Stuck,
            task_id,
            None,
            ticket_id,
            Some(self.cycles),
            message.clone(),
            ticket_id.map(|ticket_id| format!("harness ticket get {} --output json", ticket_id)),
        ))?;
        let next_commands = match ticket_id {
            Some(ticket_id) => next_for_ticket(task_id, ticket_id),
            None => next_for_task(task_id),
        };
        Ok(self.result(CommandExit::stuck(message), next_commands))
    }

    fn failed(
        mut self,
        exit: CommandExit,
        next_commands: Vec<String>,
    ) -> HarnessResult<CommandResult> {
        self.push(progress(
            SuperviseProgressPhase::Failed,
            &self.task_id.clone(),
            None,
            None,
            Some(self.cycles),
            exit.message
                .clone()
                .unwrap_or_else(|| "supervision failed".to_string()),
            next_commands.first().cloned(),
        ))?;
        Ok(self.result(exit, next_commands))
    }

    fn cancelled(mut self, task_id: &TaskId) -> HarnessResult<CommandResult> {
        let next = format!("harness supervise {} --output json", task_id);
        self.push(progress(
            SuperviseProgressPhase::Cancelled,
            task_id,
            None,
            None,
            Some(self.cycles),
            "supervision cancelled".to_string(),
            Some(next.clone()),
        ))?;
        Ok(self.result(
            CommandExit::new(
                CommandStatus::Failed,
                1,
                Some("supervision cancelled".to_string()),
            ),
            vec![next],
        ))
    }

    fn finish_with(self, result: CommandResult) -> HarnessResult<CommandResult> {
        let next_commands = result
            .data
            .get("next_commands")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| next_for_task(&self.task_id));
        self.failed(result.exit, next_commands)
    }

    fn result(self, exit: CommandExit, next_commands: Vec<String>) -> CommandResult {
        CommandResult {
            exit,
            events: self.events,
            data: json!({
                "task_id": self.task_id.as_str(),
                "cycles": self.cycles,
                "max_cycles": self.max_cycles,
                "resolved_tickets": self.resolved_tickets,
                "resolution_ids": self.resolution_ids,
                "run_ids": self.run_ids,
                "progress_events": self.progress_events.iter().map(progress_json).collect::<Vec<_>>(),
                "next_commands": next_commands,
            }),
        }
    }
}

fn classify_result(
    result: HarnessResult<CommandResult>,
    task_id: &TaskId,
    ticket_id: Option<&TicketId>,
) -> Result<CommandResult, CommandExit> {
    result.map_err(|err| classify_error(err, Some(task_id), ticket_id))
}

fn classify_error(
    error: HarnessError,
    task_id: Option<&TaskId>,
    ticket_id: Option<&TicketId>,
) -> CommandExit {
    let text = error.to_string();
    match error {
        HarnessError::Usage(_)
        | HarnessError::InvalidId { .. }
        | HarnessError::InvalidStatus { .. }
        | HarnessError::InvalidConfig(_) => CommandExit::usage(text),
        HarnessError::SecurityPolicy(_) => CommandExit::security_blocked(text),
        HarnessError::External(_) if looks_security_blocked(&text) => {
            CommandExit::security_blocked(text)
        }
        HarnessError::External(_) if looks_provider_readiness(&text) => {
            CommandExit::doctor_failed(text)
        }
        HarnessError::Conflict(_) if looks_active_lease_conflict(&text) => {
            CommandExit::failure(text)
        }
        HarnessError::Conflict(_) if ticket_id.is_some() => CommandExit::stuck(text),
        HarnessError::Conflict(_) if task_id.is_some() => CommandExit::failure(text),
        HarnessError::NotFound { .. } | HarnessError::External(_) | HarnessError::Conflict(_) => {
            CommandExit::failure(text)
        }
    }
}

fn looks_provider_readiness(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("provider ")
        || lower.contains("api key")
        || lower.contains("auth")
        || lower.contains("model")
        || lower.contains("timeout")
}

fn looks_security_blocked(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("security policy") || lower.contains("provider url rejected")
}

fn looks_active_lease_conflict(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("already has a non-expired lease")
}

fn ticket_from_data(data: &Value) -> Option<TicketId> {
    data.get("ticket_id")
        .and_then(Value::as_str)
        .and_then(|value| TicketId::parse(value).ok())
}

fn progress(
    phase: SuperviseProgressPhase,
    task_id: &TaskId,
    run_id: Option<&RunId>,
    ticket_id: Option<&TicketId>,
    cycle: Option<u32>,
    message: String,
    next_command: Option<String>,
) -> SuperviseProgressEvent {
    SuperviseProgressEvent {
        phase,
        task_id: Some(task_id.clone()),
        run_id: run_id.cloned(),
        ticket_id: ticket_id.cloned(),
        cycle,
        message,
        next_command,
    }
}

fn command_event(event: &SuperviseProgressEvent) -> CommandEvent {
    let level = match event.phase {
        SuperviseProgressPhase::Failed
        | SuperviseProgressPhase::Stuck
        | SuperviseProgressPhase::Cancelled => CommandEventLevel::Warn,
        _ => CommandEventLevel::Info,
    };
    CommandEvent::supervise_progress(event.clone(), level)
}

fn progress_json(event: &SuperviseProgressEvent) -> Value {
    event.to_json()
}

fn next_for_task(task_id: &TaskId) -> Vec<String> {
    vec![
        format!("harness task get {} --output json", task_id),
        format!("harness supervise {} --output json", task_id),
    ]
}

fn next_for_ticket(task_id: &TaskId, ticket_id: &TicketId) -> Vec<String> {
    vec![
        format!("harness ticket get {} --output json", ticket_id),
        format!(
            "harness resume {} --ticket {} --output json",
            task_id, ticket_id
        ),
        format!("harness supervise {} --output json", task_id),
    ]
}

fn push_unique(items: &mut Vec<String>, item: &str) {
    if !items.iter().any(|existing| existing == item) {
        items.push(item.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{HarnessConfig, RunStatus, TaskStatus, TicketStatus};
    use crate::providers::{FakeModelProvider, ProviderErrorKind};
    use crate::runtime::{CommandRuntime, CooperativeCancellationToken, JsonSink};
    use crate::service::DefaultHarnessService;
    use crate::state::{SqliteTaskStore, TaskStore};
    use crate::workspace::{
        CommandOutput, CommandRunner, CommandSpec, PatchApplyResult, PatchCheck, PatchCheckResult,
        RecordedWorktree, WorkspaceManager, WorktreeInfo, WorktreeRequest,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    #[test]
    fn ready_task_supervises_to_complete() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        fixture.ollama.push_text(diff_response());

        let result = fixture
            .orchestrator
            .supervise_task(&task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(result.exit.exit_code, 0);
        assert_eq!(
            fixture.store.get_task(&task.id).unwrap().status,
            TaskStatus::Complete
        );
        assert_eq!(fixture.ollama.requests().len(), 1);
        assert!(result.data["next_commands"].as_array().unwrap().is_empty());
    }

    #[test]
    fn create_and_supervise_creates_task_then_completes() {
        let fixture = Fixture::new(1, 300);
        fixture.ollama.push_text(diff_response());

        let result = fixture
            .orchestrator
            .create_and_supervise_task(SuperviseCreateOptions::new(
                "Fix",
                "Make validation pass",
                vec!["cargo test".to_string()],
            ))
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(fixture.store.list_tasks(None).unwrap().len(), 1);
        assert_eq!(fixture.ollama.requests().len(), 1);
    }

    #[test]
    fn stuck_ticket_resolves_resumes_and_completes() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        fixture
            .ollama
            .push_text("STUCK\nreason: need advice\nquestion: What next?");
        fixture.openai.push_text("Use the obvious fix.");
        fixture.ollama.push_text(diff_response());

        let result = fixture
            .orchestrator
            .supervise_task(&task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(fixture.openai.requests().len(), 1);
        assert_eq!(fixture.ollama.requests().len(), 2);
        assert_eq!(result.data["cycles"], 1);
        let resolved_tickets = result.data["resolved_tickets"].as_array().unwrap();
        let resolution_ids = result.data["resolution_ids"].as_array().unwrap();
        assert_eq!(resolved_tickets.len(), 1);
        assert_eq!(resolution_ids.len(), 1);
        assert_eq!(
            fixture.store.get_task(&task.id).unwrap().status,
            TaskStatus::Complete
        );
        let tickets = fixture.store.list_tickets(Some(&task.id), None).unwrap();
        let resolution = fixture
            .store
            .latest_unconsumed_resolution_for_ticket(&tickets[0].id)
            .unwrap();
        assert!(resolution.is_none(), "resume should consume the resolution");
    }

    #[test]
    fn json_supervise_runtime_stderr_is_only_progress_ndjson() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        let task_id = task.id.to_string();
        fixture
            .ollama
            .push_text("STUCK\nreason: need advice\nquestion: What next?");
        fixture.openai.push_text("Use the obvious fix.");
        fixture.ollama.push_text(diff_response());

        let service = DefaultHarnessService::new(fixture.orchestrator);
        let runtime = CommandRuntime::new(&service);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut sink = JsonSink::new(&mut stdout, &mut stderr, false);

        let exit = runtime.dispatch(
            ["supervise", task_id.as_str(), "--output", "json"],
            &mut sink,
        );

        assert_eq!(exit.status, CommandStatus::Complete);
        let stdout = String::from_utf8(stdout).unwrap();
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(!stderr.contains("info:"), "{stderr}");
        assert!(!stderr.contains("warn:"), "{stderr}");
        assert!(!stderr.contains("error:"), "{stderr}");

        let stdout_lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(stdout_lines.len(), 1, "{stdout:?}");
        let final_object: Value = serde_json::from_str(stdout_lines[0]).unwrap();
        assert_eq!(final_object["status"], "complete");
        assert_eq!(final_object["data"]["task_id"], task_id);

        let events = stderr
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect(line))
            .collect::<Vec<_>>();
        assert!(!events.is_empty(), "expected supervisor progress events");
        assert!(
            events
                .iter()
                .all(|event| event["event"] == "supervise.phase"),
            "{events:#?}"
        );
        assert!(
            events.iter().any(|event| event["phase"] == "complete"),
            "{events:#?}"
        );
    }

    #[test]
    fn resolved_unconsumed_ticket_resumes_without_resolving_again() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        let ticket = fixture.create_resolved_unconsumed_ticket(&task.id);
        let resolution_id = fixture
            .store
            .latest_unconsumed_resolution_for_ticket(&ticket)
            .unwrap()
            .unwrap()
            .id;
        fixture.ollama.push_text(diff_response());
        let openai_requests = fixture.openai.requests().len();

        let result = fixture
            .orchestrator
            .supervise_task(&task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(result.data["resolved_tickets"][0], ticket.as_str());
        assert_eq!(result.data["resolution_ids"][0], resolution_id.as_str());
        assert_eq!(fixture.openai.requests().len(), openai_requests);
        assert!(
            fixture
                .store
                .latest_unconsumed_resolution_for_ticket(&ticket)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn persisted_cycle_cap_stops_before_second_ticket_resolution() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        fixture
            .ollama
            .push_text("STUCK\nreason: first\nquestion: What next?");
        fixture.openai.push_text("Try once.");
        fixture
            .ollama
            .push_text("STUCK\nreason: second\nquestion: What next?");

        let result = fixture
            .orchestrator
            .supervise_task(&task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Stuck);
        assert_eq!(result.exit.exit_code, 10);
        assert_eq!(fixture.openai.requests().len(), 1);
        assert_eq!(fixture.openai.pending_steps(), 0);
        let tickets = fixture.store.list_tickets(Some(&task.id), None).unwrap();
        let unresolved_ticket = tickets
            .iter()
            .find(|ticket| ticket.status == TicketStatus::Open)
            .unwrap();
        let resolved_tickets = result.data["resolved_tickets"].as_array().unwrap();
        let resolution_ids = result.data["resolution_ids"].as_array().unwrap();
        assert_eq!(resolved_tickets.len(), 1);
        assert_eq!(resolution_ids.len(), 1);
        assert!(
            !resolved_tickets
                .iter()
                .any(|ticket| ticket.as_str() == Some(unresolved_ticket.id.as_str()))
        );
        let latest = fixture
            .store
            .latest_run_for_task(&task.id)
            .unwrap()
            .unwrap();
        assert_eq!(latest.status, RunStatus::Stuck);
        assert_eq!(latest.escalation_cycle, 1);
    }

    #[test]
    fn resolving_and_failed_tickets_are_retried_by_supervisor() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        let ticket = fixture.create_open_ticket(&task.id);
        let owner = "test-owner";
        fixture.store.acquire_task_lease(&task.id, owner).unwrap();
        fixture
            .store
            .transition_ticket(&ticket, TicketStatus::Open, TicketStatus::Resolving, owner)
            .unwrap();
        fixture.store.release_task_lease(&task.id, owner).unwrap();
        fixture.openai.push_text("Resolve retry.");
        fixture.ollama.push_text(diff_response());

        let result = fixture
            .orchestrator
            .supervise_task(&task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(fixture.openai.requests().len(), 1);

        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        let ticket = fixture.create_open_ticket(&task.id);
        let owner = "test-owner";
        fixture.store.acquire_task_lease(&task.id, owner).unwrap();
        fixture
            .store
            .transition_ticket(&ticket, TicketStatus::Open, TicketStatus::Resolving, owner)
            .unwrap();
        fixture
            .store
            .transition_ticket(
                &ticket,
                TicketStatus::Resolving,
                TicketStatus::Failed,
                owner,
            )
            .unwrap();
        fixture.store.release_task_lease(&task.id, owner).unwrap();
        fixture.openai.push_text("Resolve failed retry.");
        fixture.ollama.push_text(diff_response());

        let result = fixture
            .orchestrator
            .supervise_task(&task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(fixture.openai.requests().len(), 1);
    }

    #[test]
    fn active_resolving_ticket_lease_conflict_is_failure_not_stuck() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        let ticket = fixture.create_open_ticket(&task.id);
        let owner = "active-resolver";
        fixture.store.acquire_task_lease(&task.id, owner).unwrap();
        fixture
            .store
            .transition_ticket(&ticket, TicketStatus::Open, TicketStatus::Resolving, owner)
            .unwrap();

        let result = fixture
            .orchestrator
            .supervise_task(&task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(result.exit.exit_code, 1);
        assert!(
            result
                .exit
                .message
                .as_deref()
                .is_some_and(|message| message.contains("non-expired lease"))
        );
        assert_eq!(fixture.openai.requests().len(), 0);
    }

    #[test]
    fn active_and_expired_running_leases_return_stable_failure() {
        let active = Fixture::new(1, 300);
        let active_task = active.task();
        active.make_running(&active_task.id, "active-owner", false);

        let result = active
            .orchestrator
            .supervise_task(&active_task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(result.exit.exit_code, 1);
        assert!(result.data["next_commands"].as_array().unwrap().len() >= 1);

        let expired = Fixture::new(1, 1);
        let expired_task = expired.task();
        expired.make_running(&expired_task.id, "expired-owner", true);
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let result = expired
            .orchestrator
            .supervise_task(&expired_task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(result.exit.exit_code, 1);
        assert_eq!(
            expired.store.get_task(&expired_task.id).unwrap().status,
            TaskStatus::Failed
        );
    }

    #[test]
    fn provider_readiness_errors_map_to_exit_20() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        fixture.create_open_ticket(&task.id);
        fixture
            .openai
            .push_error(ProviderErrorKind::AuthFailed, "missing API key");

        let result = fixture
            .orchestrator
            .supervise_task(&task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::DoctorFailed);
        assert_eq!(result.exit.exit_code, 20);
        assert_eq!(fixture.openai.requests().len(), 1);
    }

    #[test]
    fn security_errors_map_to_exit_30() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        fixture.create_open_ticket(&task.id);
        fixture.openai.push_error(
            ProviderErrorKind::BadRequest,
            "provider URL rejected: security policy blocked operation",
        );

        let result = fixture
            .orchestrator
            .supervise_task(&task.id, SuperviseTaskOptions::default())
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::SecurityBlocked);
        assert_eq!(result.exit.exit_code, 30);
    }

    #[test]
    fn cancellation_safe_point_returns_resume_command_without_provider_call() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        let token = CooperativeCancellationToken::new();
        token.cancel();

        let result = fixture
            .orchestrator
            .supervise_task_with_cancellation(&task.id, SuperviseTaskOptions::default(), &token)
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(fixture.ollama.requests().len(), 0);
        assert_eq!(
            result.data["next_commands"][0],
            format!("harness supervise {} --output json", task.id)
        );
    }

    #[test]
    fn cancellation_before_resume_does_not_consume_resolution() {
        let fixture = Fixture::new(1, 300);
        let task = fixture.task();
        let ticket = fixture.create_resolved_unconsumed_ticket(&task.id);
        let token = CancelAfterChecks::new(1);

        let result = fixture
            .orchestrator
            .supervise_task_with_cancellation(&task.id, SuperviseTaskOptions::default(), &token)
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert!(
            fixture
                .store
                .latest_unconsumed_resolution_for_ticket(&ticket)
                .unwrap()
                .is_some()
        );
        assert_eq!(fixture.ollama.requests().len(), 1);
    }

    struct CancelAfterChecks {
        remaining_false: AtomicUsize,
    }

    impl CancelAfterChecks {
        fn new(remaining_false: usize) -> Self {
            Self {
                remaining_false: AtomicUsize::new(remaining_false),
            }
        }
    }

    impl CancellationToken for CancelAfterChecks {
        fn is_cancelled(&self) -> bool {
            self.remaining_false
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                    value.checked_sub(1)
                })
                .is_err()
        }
    }

    struct Fixture {
        orchestrator: RunOrchestrator,
        store: Arc<SqliteTaskStore>,
        ollama: Arc<FakeModelProvider>,
        openai: Arc<FakeModelProvider>,
        _temp: TempDir,
    }

    impl Fixture {
        fn new(max_cycles: u32, lease_ttl_secs: i64) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let workspace = Arc::new(FakeWorkspace {
                repo_root: temp.path().join("repo").to_string_lossy().into_owned(),
                worktree_root: temp.path().join("worktrees").to_string_lossy().into_owned(),
            });
            let store = Arc::new(
                SqliteTaskStore::in_memory()
                    .unwrap()
                    .with_lease_ttl_secs(lease_ttl_secs),
            );
            let runner = Arc::new(FakeRunner);
            let ollama = Arc::new(FakeModelProvider::new("fake-ollama"));
            let openai = Arc::new(FakeModelProvider::new("fake-openai"));
            let mut config = HarnessConfig::default();
            config.workspace.state_dir = temp.path().join("state").to_string_lossy().into_owned();
            config.workspace.worktree_root = workspace.worktree_root.clone();
            config.orchestrator.max_attempts = 1;
            config.orchestrator.max_escalation_cycles = max_cycles;
            let orchestrator = RunOrchestrator::new(
                config,
                store.clone(),
                workspace,
                runner,
                ollama.clone(),
                openai.clone(),
            );
            Self {
                orchestrator,
                store,
                ollama,
                openai,
                _temp: temp,
            }
        }

        fn task(&self) -> crate::domain::Task {
            self.orchestrator
                .create_task(
                    "Fix".to_string(),
                    "Make validation pass".to_string(),
                    vec!["cargo test".to_string()],
                )
                .unwrap()
        }

        fn create_open_ticket(&self, task_id: &TaskId) -> TicketId {
            self.ollama
                .push_text("STUCK\nreason: need advice\nquestion: What next?");
            let result = self
                .orchestrator
                .run_task(
                    task_id,
                    TaskRunOptions {
                        runtime: Default::default(),
                        max_attempts: None,
                        model: None,
                    },
                )
                .unwrap();
            assert_eq!(result.exit.status, CommandStatus::Stuck);
            ticket_from_data(&result.data).unwrap()
        }

        fn create_resolved_unconsumed_ticket(&self, task_id: &TaskId) -> TicketId {
            let ticket = self.create_open_ticket(task_id);
            self.openai.push_text("Use the obvious fix.");
            let result = self
                .orchestrator
                .resolve_ticket(
                    &ticket,
                    TicketResolveOptions {
                        runtime: Default::default(),
                        model: None,
                    },
                )
                .unwrap();
            assert_eq!(result.exit.status, CommandStatus::Complete);
            assert!(
                self.store
                    .latest_unconsumed_resolution_for_ticket(&ticket)
                    .unwrap()
                    .is_some()
            );
            ticket
        }

        fn make_running(&self, task_id: &TaskId, owner: &str, insert_run: bool) {
            self.store.acquire_task_lease(task_id, owner).unwrap();
            self.store
                .transition_task(task_id, TaskStatus::Ready, TaskStatus::Running, owner)
                .unwrap();
            if insert_run {
                let task = self.store.get_task(task_id).unwrap();
                self.store
                    .insert_run(
                        crate::domain::Run {
                            id: RunId::parse("run_01ARZ3NDEKTSV4RRFFQ69G5FAA").unwrap(),
                            task_id: task_id.clone(),
                            parent_run_id: None,
                            status: RunStatus::Running,
                            repo_root: task.repo_root,
                            base_ref: Some("HEAD".to_string()),
                            base_commit: "base".to_string(),
                            dirty_state_summary: None,
                            current_phase: Some("attempt".to_string()),
                            escalation_cycle: 0,
                            started_at: "1".to_string(),
                            finished_at: None,
                            final_diff_path: None,
                            last_error: None,
                        },
                        owner,
                    )
                    .unwrap();
            }
        }
    }

    #[derive(Debug)]
    struct FakeWorkspace {
        repo_root: String,
        worktree_root: String,
    }

    impl WorkspaceManager for FakeWorkspace {
        fn discover_repo_root(&self, _repo: Option<&str>) -> HarnessResult<String> {
            Ok(self.repo_root.clone())
        }

        fn source_is_dirty(&self, _repo_root: &str) -> HarnessResult<bool> {
            Ok(false)
        }

        fn resolve_base_commit(
            &self,
            _repo_root: &str,
            _base_ref: Option<&str>,
        ) -> HarnessResult<String> {
            Ok("base".to_string())
        }

        fn ensure_task_worktree(&self, request: WorktreeRequest) -> HarnessResult<WorktreeInfo> {
            let path = request
                .recorded
                .map(|recorded| recorded.path)
                .unwrap_or_else(|| {
                    let path = PathBuf::from(&self.worktree_root).join(request.task_id.as_str());
                    std::fs::create_dir_all(&path).unwrap();
                    path.to_string_lossy().into_owned()
                });
            Ok(WorktreeInfo {
                path,
                branch: format!("harness/{}", request.task_id.as_str()),
                base_ref: request.base_ref.unwrap_or_else(|| "HEAD".to_string()),
                base_commit: "base".to_string(),
                head: "head".to_string(),
            })
        }

        fn verify_recorded_worktree(
            &self,
            _repo_root: &str,
            recorded: &RecordedWorktree,
        ) -> HarnessResult<WorktreeInfo> {
            Ok(WorktreeInfo {
                path: recorded.path.clone(),
                branch: recorded.branch.clone(),
                base_ref: recorded
                    .base_ref
                    .clone()
                    .unwrap_or_else(|| "HEAD".to_string()),
                base_commit: recorded
                    .base_commit
                    .clone()
                    .unwrap_or_else(|| "base".to_string()),
                head: "head".to_string(),
            })
        }

        fn capture_diff(&self, _worktree_path: &str, _run_id: &RunId) -> HarnessResult<String> {
            Ok(String::new())
        }

        fn check_patch(&self, _patch: PatchCheck) -> HarnessResult<PatchCheckResult> {
            Ok(PatchCheckResult {
                files_changed: vec!["src/lib.rs".to_string()],
                stderr: String::new(),
            })
        }

        fn apply_patch(&self, patch: PatchCheck) -> HarnessResult<PatchApplyResult> {
            Ok(PatchApplyResult {
                check: PatchCheckResult {
                    files_changed: vec![patch.diff],
                    stderr: String::new(),
                },
                stderr: String::new(),
            })
        }

        fn cleanup_task_worktree(&self, _task_id: &TaskId, _force: bool) -> HarnessResult<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FakeRunner;

    impl CommandRunner for FakeRunner {
        fn run_validation(&self, _spec: CommandSpec) -> HarnessResult<CommandOutput> {
            Ok(CommandOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                duration_ms: 1,
                timed_out: false,
                truncated: false,
                truncated_bytes: 0,
            })
        }

        fn run_shell_escape(&self, spec: CommandSpec) -> HarnessResult<CommandOutput> {
            self.run_validation(spec)
        }
    }

    fn diff_response() -> &'static str {
        "```diff\ndiff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n```"
    }
}
