use crate::HarnessError;
use crate::HarnessResult;
use crate::domain::{
    ObjectiveAcceptanceStatus, ObjectiveArtifactId, ObjectiveEventId, ObjectiveId,
    ObjectiveResolverAttemptId, ObjectiveStatus, ObjectiveValidationReviewStatus,
    PlannerExchangeId, PlannerExchangeKind, RunId, Task, TaskId, TaskStatus, TicketId,
    TicketResolution, TicketResolutionId, TicketStatus,
};
use crate::orchestrator::RunOrchestrator;
use crate::orchestrator::supervisor_state::{
    SupervisorTicketSelection, select_ticket_for_latest_stuck_run,
};
use crate::planner::{
    ContextBudget, ContextBudgetClass, ContextPackRequest, ContextSection, TicketResolverResponse,
    TicketResolverStatuses, TicketResolverTaskDetails, TicketResolverTicketDetails,
    build_ticket_resolver_request, pack_context, parse_ticket_resolver_response,
};
use crate::prompts::build_ticket_resolver_prompt;
use crate::providers::ModelRequest;
use crate::runtime::{
    CancellationToken, CommandEvent, CommandEventLevel, CommandExit, CommandResult, CommandStatus,
    ObjectiveProgressKind, ObjectiveProgressPhase, ObjectiveSuperviseOptions,
    ObjectiveValidateOptions, OutputSink, SuperviseTaskOptions,
};
use crate::security::{ValidationCommandClassification, ValidationCommandPolicy};
use crate::state::{
    NewGeneratedTask, NewObjectiveResolverAttempt, ObjectiveArtifact, ObjectiveEvent,
    ObjectiveStatusUpdate, PlannerExchange, TaskStore,
};
use crate::workspace::{CommandOutput, CommandSpec, CommandStdin};
use std::collections::BTreeMap;
use std::fs;

use serde_json::json;

impl RunOrchestrator {
    pub fn supervise_objective(
        &self,
        objective_id: &ObjectiveId,
        options: ObjectiveSuperviseOptions,
    ) -> HarnessResult<CommandResult> {
        self.supervise_objective_with_cancellation(objective_id, options, &NeverCancelled)
    }

    pub fn supervise_objective_with_cancellation(
        &self,
        objective_id: &ObjectiveId,
        options: ObjectiveSuperviseOptions,
        cancellation: &dyn CancellationToken,
    ) -> HarnessResult<CommandResult> {
        self.supervise_objective_with_cancellation_and_sink(
            objective_id,
            options,
            cancellation,
            None,
        )
    }

    pub fn supervise_objective_streaming(
        &self,
        objective_id: &ObjectiveId,
        options: ObjectiveSuperviseOptions,
        sink: &mut dyn OutputSink,
    ) -> HarnessResult<CommandResult> {
        self.supervise_objective_with_cancellation_and_sink(
            objective_id,
            options,
            &NeverCancelled,
            Some(sink),
        )
    }

    fn supervise_objective_with_cancellation_and_sink(
        &self,
        objective_id: &ObjectiveId,
        options: ObjectiveSuperviseOptions,
        cancellation: &dyn CancellationToken,
        sink: Option<&mut dyn OutputSink>,
    ) -> HarnessResult<CommandResult> {
        let owner = objective_owner(objective_id);
        self.store
            .acquire_objective_monitor_lease(objective_id, &owner)?;
        let result =
            self.supervise_objective_with_lease(objective_id, options, &owner, cancellation, sink);
        let release = self
            .store
            .release_objective_monitor_lease(objective_id, &owner);
        match (result, release) {
            (Ok(result), Ok(())) => Ok(result),
            (Ok(_), Err(err)) | (Err(err), _) => Err(err),
        }
    }

    fn supervise_objective_with_lease(
        &self,
        objective_id: &ObjectiveId,
        options: ObjectiveSuperviseOptions,
        owner: &str,
        cancellation: &dyn CancellationToken,
        mut sink: Option<&mut dyn OutputSink>,
    ) -> HarnessResult<CommandResult> {
        if cancellation.is_cancelled() {
            return self.cancel_objective_monitor(objective_id);
        }
        let max_cycles = options.max_cycles.unwrap_or(16);
        let mut result = CommandResult::with_data(
            CommandExit::success(),
            json!({
                "objective_id": objective_id.as_str(),
                "status": "running",
                "terminal": false,
                "task_ids": [],
                "open_ticket_ids": [],
                "validation": {
                    "status": "pending",
                    "commands_run": 0,
                    "commands_skipped": 0,
                },
                "next": format!("harness objective get {}", objective_id.as_str()),
            }),
        )
        .with_event(event(
            ObjectiveProgressKind::SupervisionStarted,
            objective_id,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
            "supervising objective",
        ));
        stream_new_events(&mut sink, &result.events, 0)?;

        self.store.update_objective_status(
            objective_id,
            None,
            ObjectiveStatusUpdate {
                status: Some(ObjectiveStatus::Running),
                updated_at: Some(crate::orchestrator::orchestrator_now()),
                ..ObjectiveStatusUpdate::default()
            },
        )?;

        for _cycle in 0..max_cycles {
            if cancellation.is_cancelled() {
                return self.cancel_objective_monitor(objective_id);
            }
            self.store
                .refresh_objective_monitor_lease(objective_id, owner)?;
            let cycle = match sink.as_mut() {
                Some(sink) => self.run_one_objective_cycle_streaming(
                    objective_id,
                    &options,
                    Some(&mut **sink),
                )?,
                None => self.run_one_objective_cycle_streaming(objective_id, &options, None)?,
            };
            match cycle {
                ObjectiveCycleResult::Progress(events) => {
                    let skip = if sink.is_some() && starts_with_worker_started(&events) {
                        1
                    } else {
                        0
                    };
                    stream_new_events(&mut sink, &events, skip)?;
                    result.events.extend(events);
                }
                ObjectiveCycleResult::Terminal(mut terminal) => {
                    let skip = if sink.is_some() && starts_with_worker_started(&terminal.events) {
                        1
                    } else {
                        0
                    };
                    stream_new_events(&mut sink, &terminal.events, skip)?;
                    result.events.extend(std::mem::take(&mut terminal.events));
                    terminal.events = result.events;
                    return Ok(terminal);
                }
            }
        }

        self.store.update_objective_status(
            objective_id,
            None,
            ObjectiveStatusUpdate {
                status: Some(ObjectiveStatus::Blocked),
                summary: Some("objective supervise reached max cycles".to_string()),
                updated_at: Some(crate::orchestrator::orchestrator_now()),
                ..ObjectiveStatusUpdate::default()
            },
        )?;
        Ok(CommandResult::with_data(
            CommandExit::failure("objective supervise reached max cycles"),
            json!({
                "objective_id": objective_id.as_str(),
                "status": ObjectiveStatus::Blocked.as_str(),
                "terminal": true,
                "exit_reason": "max_cycles_exhausted",
                "task_ids": [],
                "open_ticket_ids": [],
                "validation": {
                    "status": "skipped",
                    "commands_run": 0,
                    "commands_skipped": 0,
                },
                "next": format!("harness objective get {}", objective_id.as_str()),
            }),
        )
        .with_event(event(
            ObjectiveProgressKind::Blocked,
            objective_id,
            ObjectiveProgressPhase::Blocked,
            ObjectiveStatus::Blocked,
            "objective supervise reached max cycles",
        )))
    }

    pub fn run_one_objective_cycle(
        &self,
        objective_id: &ObjectiveId,
        options: &ObjectiveSuperviseOptions,
    ) -> HarnessResult<ObjectiveCycleResult> {
        self.run_one_objective_cycle_streaming(objective_id, options, None)
    }

    fn run_one_objective_cycle_streaming(
        &self,
        objective_id: &ObjectiveId,
        options: &ObjectiveSuperviseOptions,
        mut sink: Option<&mut dyn OutputSink>,
    ) -> HarnessResult<ObjectiveCycleResult> {
        self.recover_expired_objective_task_leases(objective_id)?;
        if let Some(task) = self.store.next_ready_objective_task(objective_id)? {
            let Some(max_attempts) =
                self.remaining_worker_attempts_or_block(objective_id, &task.id)?
            else {
                return self.block_objective(
                    objective_id,
                    format!("generated task {} exhausted worker attempts", task.id),
                );
            };
            let worker_model = options
                .worker_model
                .clone()
                .unwrap_or_else(|| self.config.providers.ollama.default_model.clone());
            let mut events = vec![task_event(
                ObjectiveProgressKind::WorkerStarted,
                objective_id,
                &task.id,
                ObjectiveProgressPhase::Running,
                ObjectiveStatus::Running,
                format!(
                    "running generated task {} with local Ollama model {}",
                    task.id, worker_model
                ),
            )];
            self.persist_objective_event(
                objective_id,
                ObjectiveProgressKind::WorkerStarted,
                format!(
                    "running generated task {} with local Ollama model {}",
                    task.id, worker_model
                ),
                json!({"task_id": task.id.as_str(), "model": worker_model}),
            )?;
            stream_new_events(&mut sink, &events, 0)?;
            let supervise_options = SuperviseTaskOptions {
                runtime: options.runtime.clone(),
                ticket_id: None,
                max_attempts: Some(limit_worker_attempts(
                    options.max_worker_attempts,
                    max_attempts,
                )),
                model: options.worker_model.clone(),
                ticket_model: options.ticket_model.clone(),
                max_cycles: Some(0),
            };
            let supervised = self.supervise_task(&task.id, supervise_options)?;
            self.record_objective_worker_attempts_from_result(objective_id, &task.id, &supervised)?;
            match supervised.exit.status {
                CommandStatus::Complete => {
                    let completed = self.store.get_task(&task.id)?;
                    let branch = completed.branch.as_ref().ok_or_else(|| {
                        HarnessError::Conflict(format!(
                            "completed objective task {} has no task branch",
                            completed.id
                        ))
                    })?;
                    self.workspace
                        .fast_forward_repo(&completed.repo_root, branch)?;
                    let completed_event = task_event(
                        ObjectiveProgressKind::WorkerCompleted,
                        objective_id,
                        &task.id,
                        ObjectiveProgressPhase::Running,
                        ObjectiveStatus::Running,
                        worker_completed_message(&task, &supervised, self.store.as_ref())?,
                    );
                    self.persist_objective_event(
                        objective_id,
                        ObjectiveProgressKind::WorkerCompleted,
                        completed_event
                            .objective_progress
                            .as_ref()
                            .map(|progress| progress.message.clone())
                            .unwrap_or_else(|| format!("generated task {} completed", task.id)),
                        json!({"task_id": task.id.as_str()}),
                    )?;
                    events.push(completed_event);
                    Ok(ObjectiveCycleResult::Progress(events))
                }
                CommandStatus::Stuck => {
                    self.resolve_and_resume_objective_task(objective_id, &task.id, options, events)
                }
                _ => self.fail_objective(
                    objective_id,
                    format!("generated task {} failed", task.id),
                    supervised.exit,
                ),
            }
        } else {
            let counts = self
                .store
                .active_objective_task_status_counts(objective_id)?;
            if counts.total > 0 && counts.complete == counts.total {
                return self.run_objective_validation_or_repair(objective_id, options);
            }
            if counts.stuck > 0 {
                if let Some(task) = self.store.next_stuck_objective_task(objective_id)? {
                    return self.resolve_and_resume_objective_task(
                        objective_id,
                        &task.id,
                        options,
                        vec![task_event(
                            ObjectiveProgressKind::TicketDetected,
                            objective_id,
                            &task.id,
                            ObjectiveProgressPhase::Running,
                            ObjectiveStatus::Running,
                            format!("generated task {} is stuck", task.id),
                        )],
                    );
                }
                return self.block_objective(objective_id, "generated task is stuck".to_string());
            }
            if counts.failed > 0 {
                return self.fail_objective(
                    objective_id,
                    "generated task failed".to_string(),
                    CommandExit::failure("generated task failed"),
                );
            }
            if counts.running > 0 {
                return Ok(ObjectiveCycleResult::Progress(vec![event(
                    ObjectiveProgressKind::WorkerStarted,
                    objective_id,
                    ObjectiveProgressPhase::Running,
                    ObjectiveStatus::Running,
                    "generated task is still running",
                )]));
            }
            Ok(ObjectiveCycleResult::Progress(Vec::new()))
        }
    }

    pub fn validate_objective(
        &self,
        objective_id: &ObjectiveId,
        options: ObjectiveValidateOptions,
    ) -> HarnessResult<CommandResult> {
        let commands = self
            .store
            .list_active_objective_validation_commands(objective_id)?;
        if options.dry_run {
            return Ok(CommandResult::with_data(
                CommandExit::success(),
                json!({
                    "objective_id": objective_id.as_str(),
                    "dry_run": true,
                    "validation": {
                        "status": "pending",
                        "commands_run": 0,
                        "commands_skipped": commands.len(),
                    },
                    "commands": commands.iter().map(|command| json!({
                        "command": command.command,
                        "review_status": command.review_status.as_str(),
                        "review_reason": command.review_reason,
                    })).collect::<Vec<_>>(),
                }),
            ));
        }
        let counts = self
            .store
            .active_objective_task_status_counts(objective_id)?;
        if counts.total > 0 && counts.complete != counts.total {
            return Ok(CommandResult::with_data(
                CommandExit::failure("objective tasks are not complete"),
                json!({
                    "objective_id": objective_id.as_str(),
                    "validation": {
                        "status": "skipped",
                        "commands_run": 0,
                        "commands_skipped": commands.len(),
                    },
                    "reason": "objective_tasks_incomplete",
                }),
            ));
        }
        match self.run_objective_validation_or_repair(
            objective_id,
            &ObjectiveSuperviseOptions {
                runtime: options.runtime,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: None,
                max_cycles: None,
            },
        )? {
            ObjectiveCycleResult::Terminal(result) => Ok(result),
            ObjectiveCycleResult::Progress(events) => Ok(CommandResult {
                exit: CommandExit::new(
                    CommandStatus::Complete,
                    0,
                    Some("objective validation made progress".to_string()),
                ),
                events,
                data: json!({
                    "objective_id": objective_id.as_str(),
                    "validation": {
                        "status": "failed",
                        "commands_run": 1,
                        "commands_skipped": 0,
                    },
                    "next": format!("harness objective supervise {}", objective_id.as_str()),
                }),
            }),
        }
    }

    fn recover_expired_objective_task_leases(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<()> {
        for task_id in self.store.list_active_objective_task_ids(objective_id)? {
            self.store.recover_expired_supervisor_leases(&task_id)?;
        }
        Ok(())
    }

    fn remaining_worker_attempts_or_block(
        &self,
        objective_id: &ObjectiveId,
        task_id: &TaskId,
    ) -> HarnessResult<Option<u32>> {
        let objective_task = self.store.get_objective_task(objective_id, task_id)?;
        let remaining = objective_task
            .worker_attempt_budget
            .saturating_sub(objective_task.worker_attempts_used);
        Ok((remaining > 0).then_some(remaining))
    }

    fn run_objective_validation_or_repair(
        &self,
        objective_id: &ObjectiveId,
        options: &ObjectiveSuperviseOptions,
    ) -> HarnessResult<ObjectiveCycleResult> {
        let objective = self.store.get_objective(objective_id)?;
        let commands = self
            .store
            .list_active_objective_validation_commands(objective_id)?;
        if commands.is_empty() {
            return self.block_objective(
                objective_id,
                "objective has no validation commands".to_string(),
            );
        }
        if let Some(command) = commands
            .iter()
            .find(|command| command.review_status != ObjectiveValidationReviewStatus::Trusted)
        {
            return self.block_objective(
                objective_id,
                format!(
                    "objective validation command requires review: {}",
                    command.command
                ),
            );
        }
        let policy = ValidationCommandPolicy::new();
        if let Some((command, review)) = commands.iter().find_map(|command| {
            let review = policy.classify(&command.command);
            (review.classification != ValidationCommandClassification::Trusted)
                .then_some((command, review))
        }) {
            return self.block_objective(
                objective_id,
                format!(
                    "objective validation command blocked by safety policy: {} ({})",
                    command.command,
                    review.reasons.join("; ")
                ),
            );
        }

        let validation =
            self.run_objective_validation_commands(objective_id, &commands, &options.runtime)?;
        if validation.passed {
            self.store.update_active_objective_acceptance_status(
                objective_id,
                ObjectiveAcceptanceStatus::Passing,
                &crate::orchestrator::orchestrator_now(),
            )?;
            self.store.update_objective_status(
                objective_id,
                None,
                ObjectiveStatusUpdate {
                    status: Some(ObjectiveStatus::Complete),
                    summary: Some("Objective acceptance validation passed".to_string()),
                    updated_at: Some(crate::orchestrator::orchestrator_now()),
                    ..ObjectiveStatusUpdate::default()
                },
            )?;
            return Ok(ObjectiveCycleResult::Terminal(
                CommandResult::with_data(
                    CommandExit::success(),
                    json!({
                        "objective_id": objective_id.as_str(),
                        "status": ObjectiveStatus::Complete.as_str(),
                        "terminal": true,
                        "exit_reason": "acceptance_passed",
                        "task_ids": [],
                        "open_ticket_ids": [],
                        "validation": {
                            "status": "passed",
                            "commands_run": validation.commands_run,
                            "commands_skipped": 0,
                        },
                        "next": format!("harness objective get {}", objective_id.as_str()),
                    }),
                )
                .with_event(event(
                    ObjectiveProgressKind::ValidationPassed,
                    objective_id,
                    ObjectiveProgressPhase::Complete,
                    ObjectiveStatus::Complete,
                    "objective acceptance validation passed",
                ))
                .with_event(event(
                    ObjectiveProgressKind::Completed,
                    objective_id,
                    ObjectiveProgressPhase::Complete,
                    ObjectiveStatus::Complete,
                    "objective complete",
                )),
            ));
        }

        self.store.update_active_objective_acceptance_status(
            objective_id,
            ObjectiveAcceptanceStatus::Failing,
            &crate::orchestrator::orchestrator_now(),
        )?;
        let repair = self.find_acceptance_repair_task(objective_id)?;
        if let Some(repair) = repair {
            if repair.status == TaskStatus::Complete {
                return self.block_objective(
                    objective_id,
                    "objective validation still fails after acceptance repair".to_string(),
                );
            }
            return Ok(ObjectiveCycleResult::Progress(vec![event(
                ObjectiveProgressKind::ValidationFailed,
                objective_id,
                ObjectiveProgressPhase::Running,
                ObjectiveStatus::Running,
                format!(
                    "objective validation failed; waiting on acceptance repair task {}",
                    repair.id
                ),
            )]));
        }

        let repair = self.create_acceptance_repair_task(
            objective_id,
            &objective,
            &commands
                .iter()
                .map(|command| command.command.clone())
                .collect::<Vec<_>>(),
            validation,
            options,
        )?;
        Ok(ObjectiveCycleResult::Progress(vec![
            event(
                ObjectiveProgressKind::ValidationFailed,
                objective_id,
                ObjectiveProgressPhase::Running,
                ObjectiveStatus::Running,
                "objective validation failed",
            ),
            event(
                ObjectiveProgressKind::RepairTaskCreated,
                objective_id,
                ObjectiveProgressPhase::Running,
                ObjectiveStatus::Running,
                format!("created acceptance repair task {}", repair.task_id),
            ),
        ]))
    }

    fn run_objective_validation_commands(
        &self,
        objective_id: &ObjectiveId,
        commands: &[crate::state::ObjectiveValidationCommand],
        runtime: &crate::runtime::RuntimeOptions,
    ) -> HarnessResult<ObjectiveValidationResult> {
        let repo_root = self
            .workspace
            .discover_repo_root(runtime.repo.as_deref().and_then(|path| path.to_str()))?;
        let policy = ValidationCommandPolicy::new();
        self.persist_objective_event(
            objective_id,
            ObjectiveProgressKind::ValidationStarted,
            "running objective validation".to_string(),
            json!({"commands": commands.iter().map(|command| command.command.as_str()).collect::<Vec<_>>()}),
        )?;
        for (idx, command) in commands.iter().enumerate() {
            let review = policy.classify(&command.command);
            if review.classification != ValidationCommandClassification::Trusted {
                return Err(HarnessError::SecurityPolicy(format!(
                    "objective validation command is not trusted: {}",
                    command.command
                )));
            }
            let output = self.runner.run_validation(CommandSpec {
                command: command.command.clone(),
                cwd: repo_root.clone(),
                shell_path: self.config.command.shell_path.clone(),
                env: BTreeMap::new(),
                timeout_seconds: self.config.orchestrator.validation_timeout_seconds,
                max_output_bytes: self.config.orchestrator.max_validation_output_bytes,
                stdin: CommandStdin::Null,
                kill_process_group_on_timeout: self.config.command.kill_process_group_on_timeout,
            });
            match output {
                Ok(output) if output.exit_code == Some(0) && !output.timed_out => {}
                Ok(output) => {
                    let log = validation_log(&command.command, &output);
                    let artifact_name = unique_validation_failure_artifact_name()?;
                    let artifact = self.write_objective_artifact(
                        objective_id,
                        None,
                        None,
                        "objective_validation_failure",
                        &artifact_name,
                        &log,
                    )?;
                    self.store.insert_objective_artifact(artifact.clone())?;
                    self.persist_objective_event(
                        objective_id,
                        ObjectiveProgressKind::ValidationFailed,
                        format!("objective validation command failed: {}", command.command),
                        json!({
                            "command": command.command,
                            "artifact_path": artifact.path,
                            "exit_code": output.exit_code,
                            "timed_out": output.timed_out,
                        }),
                    )?;
                    return Ok(ObjectiveValidationResult {
                        passed: false,
                        commands_run: idx as u32 + 1,
                        failing_command: Some(command.command.clone()),
                        failure_log_path: Some(artifact.path),
                    });
                }
                Err(error) => {
                    let artifact_name = unique_validation_failure_artifact_name()?;
                    let artifact = self.write_objective_artifact(
                        objective_id,
                        None,
                        None,
                        "objective_validation_failure",
                        &artifact_name,
                        &error.to_string(),
                    )?;
                    self.store.insert_objective_artifact(artifact.clone())?;
                    self.persist_objective_event(
                        objective_id,
                        ObjectiveProgressKind::ValidationFailed,
                        format!("objective validation command errored: {}", command.command),
                        json!({
                            "command": command.command,
                            "artifact_path": artifact.path,
                            "error": error.to_string(),
                        }),
                    )?;
                    return Ok(ObjectiveValidationResult {
                        passed: false,
                        commands_run: idx as u32 + 1,
                        failing_command: Some(command.command.clone()),
                        failure_log_path: Some(artifact.path),
                    });
                }
            }
        }
        Ok(ObjectiveValidationResult {
            passed: true,
            commands_run: commands.len() as u32,
            failing_command: None,
            failure_log_path: None,
        })
    }

    fn find_acceptance_repair_task(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<Option<Task>> {
        for task_id in self.store.list_active_objective_task_ids(objective_id)? {
            let objective_task = self.store.get_objective_task(objective_id, &task_id)?;
            if objective_task.task_key == "acceptance_repair" {
                return self.store.get_task(&task_id).map(Some);
            }
        }
        Ok(None)
    }

    fn create_acceptance_repair_task(
        &self,
        objective_id: &ObjectiveId,
        objective: &crate::state::Objective,
        validations: &[String],
        validation: ObjectiveValidationResult,
        options: &ObjectiveSuperviseOptions,
    ) -> HarnessResult<crate::state::ObjectiveTask> {
        let repo_root = self.workspace.discover_repo_root(
            options
                .runtime
                .repo
                .as_deref()
                .and_then(|path| path.to_str()),
        )?;
        let created_at = crate::orchestrator::orchestrator_now();
        let task_id = super::new_id(TaskId::PREFIX, TaskId::parse)?;
        let goal = format!(
            "Repair objective acceptance for objective {objective_id}.\n\nObjective prompt:\n{}\n\nAcceptance criteria:\n{}\n\nFailing validation command:\n{}\n\nValidation failure artifact:\n{}\n\nAfter repairing, all objective validation commands must pass.",
            objective.prompt,
            self.store
                .list_active_objective_acceptance_criteria(objective_id)?
                .into_iter()
                .map(|criterion| format!("- {}", criterion.description))
                .collect::<Vec<_>>()
                .join("\n"),
            validation
                .failing_command
                .as_deref()
                .unwrap_or("unknown validation command"),
            validation.failure_log_path.as_deref().unwrap_or("none"),
        );
        let task = Task {
            id: task_id,
            title: format!("Repair objective acceptance: {}", objective.title),
            goal,
            status: TaskStatus::Ready,
            repo_root,
            worktree_path: None,
            branch: None,
            base_ref: None,
            base_commit: None,
            last_seen_head: None,
            max_attempts: self.effective_objective_worker_attempts(options.max_worker_attempts),
            lease_owner: None,
            lease_acquired_at: None,
            lease_expires_at: None,
            heartbeat_at: None,
            lock_version: 0,
            created_at: created_at.clone(),
            updated_at: created_at.clone(),
        };
        self.store.create_objective_repair_task(
            objective_id,
            NewGeneratedTask {
                task,
                task_key: "acceptance_repair".to_string(),
                parallel_group: Some("acceptance_repair".to_string()),
                owned_paths_json: "[]".to_string(),
                sequence: u32::MAX,
                worker_attempt_budget: self
                    .effective_objective_worker_attempts(options.max_worker_attempts),
                trusted_validation_commands: validations.to_vec(),
                reviewed_validation_commands: Vec::new(),
            },
        )
    }

    fn resolve_and_resume_objective_task(
        &self,
        objective_id: &ObjectiveId,
        task_id: &TaskId,
        options: &ObjectiveSuperviseOptions,
        mut events: Vec<CommandEvent>,
    ) -> HarnessResult<ObjectiveCycleResult> {
        let Some(selection) =
            select_ticket_for_latest_stuck_run(self.store.as_ref(), task_id, None)?
        else {
            return self.block_objective(
                objective_id,
                format!(
                    "generated task {} is stuck without a retryable ticket",
                    task_id
                ),
            );
        };
        let ticket = selection.ticket.ticket().clone();
        let ticket_id = ticket.id.clone();
        let detected_message = ticket_detected_message(task_id, &ticket);
        events.push(task_ticket_event(
            ObjectiveProgressKind::TicketDetected,
            objective_id,
            task_id,
            &ticket_id,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
            detected_message.clone(),
        ));
        self.persist_objective_event(
            objective_id,
            ObjectiveProgressKind::TicketDetected,
            detected_message,
            json!({
                "task_id": task_id.as_str(),
                "ticket_id": ticket_id.as_str(),
                "blocked_on": ticket.blocked_on,
                "reason": ticket.reason,
                "question": ticket.question,
            }),
        )?;

        if !matches!(
            selection.ticket,
            SupervisorTicketSelection::ResolvedUnconsumed { .. }
        ) {
            match self.resolve_objective_ticket(objective_id, task_id, &ticket_id, options) {
                Ok(resolver_events) => events.extend(resolver_events),
                Err(error) => {
                    return self.block_objective(
                        objective_id,
                        format!("ticket {} resolver failed: {}", ticket_id, error),
                    );
                }
            }
        }

        let Some(max_attempts) = self.remaining_worker_attempts_or_block(objective_id, task_id)?
        else {
            return self.block_objective(
                objective_id,
                format!("generated task {} exhausted worker attempts", task_id),
            );
        };
        let resume_message = format!(
            "resuming generated task {} with local Ollama after ticket {}",
            task_id, ticket_id
        );
        events.push(task_event(
            ObjectiveProgressKind::WorkerResumed,
            objective_id,
            task_id,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
            resume_message.clone(),
        ));
        self.persist_objective_event(
            objective_id,
            ObjectiveProgressKind::WorkerResumed,
            resume_message,
            json!({"task_id": task_id.as_str(), "ticket_id": ticket_id.as_str()}),
        )?;
        let resumed = self.supervise_task(
            task_id,
            SuperviseTaskOptions {
                runtime: options.runtime.clone(),
                ticket_id: Some(ticket_id.clone()),
                max_attempts: Some(limit_worker_attempts(
                    options.max_worker_attempts,
                    max_attempts,
                )),
                model: options.worker_model.clone(),
                ticket_model: options.ticket_model.clone(),
                max_cycles: Some(selection.stuck_run.next_cycle),
            },
        )?;
        self.record_objective_worker_attempts_from_result(objective_id, task_id, &resumed)?;
        match resumed.exit.status {
            CommandStatus::Complete => {
                events.push(task_event(
                    ObjectiveProgressKind::WorkerCompleted,
                    objective_id,
                    task_id,
                    ObjectiveProgressPhase::Running,
                    ObjectiveStatus::Running,
                    format!(
                        "generated task {} completed after ticket resolution",
                        task_id
                    ),
                ));
                Ok(ObjectiveCycleResult::Progress(events))
            }
            CommandStatus::Stuck => {
                let new_ticket_id = resumed
                    .data
                    .get("ticket_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|id| TicketId::parse(id).ok());
                if let Some(new_ticket_id) = &new_ticket_id {
                    if new_ticket_id == &ticket_id {
                        let repeated = self
                            .store
                            .get_ticket(new_ticket_id)
                            .map(|ticket| ticket_summary(&ticket))
                            .unwrap_or_else(|_| {
                                "worker repeated the same unresolved blocker".to_string()
                            });
                        let message = format!(
                            "generated task {} repeated ticket {} after resolution: {}",
                            task_id, new_ticket_id, repeated
                        );
                        return self.block_objective_with_prior_events(
                            objective_id,
                            message,
                            events,
                        );
                    }
                    let detected_message = self
                        .store
                        .get_ticket(new_ticket_id)
                        .map(|ticket| ticket_detected_message(task_id, &ticket))
                        .unwrap_or_else(|_| {
                            format!(
                                "generated task {} opened new ticket {} after resume",
                                task_id, new_ticket_id
                            )
                        });
                    events.push(task_ticket_event(
                        ObjectiveProgressKind::TicketDetected,
                        objective_id,
                        task_id,
                        new_ticket_id,
                        ObjectiveProgressPhase::Running,
                        ObjectiveStatus::Running,
                        detected_message.clone(),
                    ));
                    self.persist_objective_event(
                        objective_id,
                        ObjectiveProgressKind::TicketDetected,
                        detected_message,
                        json!({
                            "task_id": task_id.as_str(),
                            "ticket_id": new_ticket_id.as_str(),
                        }),
                    )?;
                    return Ok(ObjectiveCycleResult::Progress(events));
                }
                let message = match &new_ticket_id {
                    Some(_) => unreachable!("new ticket returns progress above"),
                    None => format!(
                        "generated task {} remains stuck after ticket {}: {}",
                        task_id,
                        ticket_id,
                        resumed
                            .exit
                            .message
                            .as_deref()
                            .map(|message| compact_one_line(message, 220))
                            .unwrap_or_else(|| "no worker reason was reported".to_string())
                    ),
                };
                self.block_objective_with_prior_events(objective_id, message, events)
            }
            _ => self.fail_objective(
                objective_id,
                format!(
                    "generated task {} failed after ticket {}",
                    task_id, ticket_id
                ),
                resumed.exit,
            ),
        }
    }

    fn block_objective_with_prior_events(
        &self,
        objective_id: &ObjectiveId,
        reason: String,
        mut prior_events: Vec<CommandEvent>,
    ) -> HarnessResult<ObjectiveCycleResult> {
        let ObjectiveCycleResult::Terminal(mut terminal) =
            self.block_objective(objective_id, reason)?
        else {
            unreachable!("block_objective always returns a terminal result");
        };
        prior_events.extend(std::mem::take(&mut terminal.events));
        terminal.events = prior_events;
        Ok(ObjectiveCycleResult::Terminal(terminal))
    }

    fn record_objective_worker_attempts_from_result(
        &self,
        objective_id: &ObjectiveId,
        task_id: &TaskId,
        result: &CommandResult,
    ) -> HarnessResult<()> {
        let delta = result
            .data
            .get("run_ids")
            .and_then(serde_json::Value::as_array)
            .map(|runs| {
                runs.iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter_map(|run_id| RunId::parse(run_id).ok())
                    .map(|run_id| {
                        self.store
                            .list_attempts(&run_id)
                            .map(|attempts| attempts.len() as u32)
                    })
                    .try_fold(0_u32, |sum, count| count.map(|count| sum + count))
            })
            .transpose()?
            .unwrap_or(0);
        if delta > 0 {
            self.store
                .increment_objective_task_attempts_used(objective_id, task_id, delta)?;
        }
        Ok(())
    }

    fn resolve_objective_ticket(
        &self,
        objective_id: &ObjectiveId,
        task_id: &TaskId,
        ticket_id: &TicketId,
        options: &ObjectiveSuperviseOptions,
    ) -> HarnessResult<Vec<CommandEvent>> {
        let attempts = self
            .store
            .list_resolver_attempts_for_ticket(objective_id, ticket_id)?;
        if attempts.iter().any(|attempt| attempt.status == "resolved") {
            return Ok(Vec::new());
        }
        if attempts.iter().any(|attempt| attempt.status == "failed") {
            return Err(HarnessError::Conflict(format!(
                "ticket {} already has a failed objective resolver attempt",
                ticket_id
            )));
        }
        let attempt = attempts
            .iter()
            .rev()
            .find(|attempt| matches!(attempt.status.as_str(), "queued" | "resolving"))
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| {
                self.store
                    .create_resolver_attempt(NewObjectiveResolverAttempt {
                        id: super::new_id(
                            ObjectiveResolverAttemptId::PREFIX,
                            ObjectiveResolverAttemptId::parse,
                        )?,
                        objective_id: objective_id.clone(),
                        ticket_id: ticket_id.clone(),
                        attempt: attempts.len() as u32 + 1,
                        created_at: crate::orchestrator::orchestrator_now(),
                    })
            })?;
        let owner = format!("objective-resolver-{}-{}", std::process::id(), attempt.id);
        let attempt = self
            .store
            .acquire_resolver_attempt_lease(&attempt.id, &owner)?;
        let mut events = vec![task_ticket_event(
            ObjectiveProgressKind::TicketResolutionStarted,
            objective_id,
            task_id,
            ticket_id,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
            format!(
                "resolving ticket {} for generated task {}",
                ticket_id, task_id
            ),
        )];
        self.persist_objective_event(
            objective_id,
            ObjectiveProgressKind::TicketResolutionStarted,
            format!("resolving ticket {}", ticket_id),
            json!({"ticket_id": ticket_id.as_str(), "task_id": task_id.as_str()}),
        )?;
        match self.resolve_objective_ticket_with_attempt(
            objective_id,
            task_id,
            ticket_id,
            &attempt.id,
            &owner,
            options,
        ) {
            Ok(mut completed) => {
                events.append(&mut completed);
                Ok(events)
            }
            Err(error) => {
                let _ = self.store.release_resolver_attempt_lease(
                    &attempt.id,
                    &owner,
                    "failed",
                    None,
                    Some(&error.to_string()),
                );
                Err(error)
            }
        }
    }

    fn resolve_objective_ticket_with_attempt(
        &self,
        objective_id: &ObjectiveId,
        task_id: &TaskId,
        ticket_id: &TicketId,
        attempt_id: &ObjectiveResolverAttemptId,
        resolver_owner: &str,
        options: &ObjectiveSuperviseOptions,
    ) -> HarnessResult<Vec<CommandEvent>> {
        let objective = self.store.get_objective(objective_id)?;
        let task = self.store.get_task(task_id)?;
        let ticket = self.store.get_ticket(ticket_id)?;
        if ticket.task_id != *task_id {
            return Err(HarnessError::Conflict(format!(
                "ticket {} does not belong to task {}",
                ticket_id, task_id
            )));
        }
        let run = self.store.get_run(&ticket.run_id)?;
        let task_owner = super::owner(task_id);
        self.store.acquire_task_lease(task_id, &task_owner)?;
        let result = self.resolve_objective_ticket_with_task_lease(
            objective_id,
            attempt_id,
            resolver_owner,
            &task_owner,
            objective,
            task,
            ticket,
            run.id.clone(),
            options,
        );
        let release = self.store.release_task_lease(task_id, &task_owner);
        match (result, release) {
            (Ok(result), Ok(())) => Ok(result),
            (Ok(_), Err(err)) | (Err(err), _) => Err(err),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_objective_ticket_with_task_lease(
        &self,
        objective_id: &ObjectiveId,
        attempt_id: &ObjectiveResolverAttemptId,
        resolver_owner: &str,
        task_owner: &str,
        objective: crate::state::Objective,
        task: crate::domain::Task,
        mut ticket: crate::domain::Ticket,
        run_id: RunId,
        options: &ObjectiveSuperviseOptions,
    ) -> HarnessResult<Vec<CommandEvent>> {
        ticket = match ticket.status {
            TicketStatus::Open => self.store.transition_ticket(
                &ticket.id,
                TicketStatus::Open,
                TicketStatus::Resolving,
                task_owner,
            )?,
            TicketStatus::Resolving => ticket,
            TicketStatus::Resolved => return Ok(Vec::new()),
            TicketStatus::Failed => {
                return Err(HarnessError::Conflict(format!(
                    "ticket {} has already failed resolver handling",
                    ticket.id
                )));
            }
        };

        let exchange_id = super::new_id(PlannerExchangeId::PREFIX, PlannerExchangeId::parse)?;
        let model = options
            .ticket_model
            .clone()
            .or_else(|| objective.ticket_model.clone())
            .unwrap_or_else(|| self.config.providers.openai.default_model.clone());
        let criteria = self
            .store
            .list_active_objective_acceptance_criteria(objective_id)?
            .into_iter()
            .map(|criterion| self.redact_text(&criterion.description))
            .collect::<Vec<_>>();
        let validations = self
            .store
            .list_validation_commands(&task.id)?
            .into_iter()
            .map(|command| self.redact_text(&command.command))
            .collect::<Vec<_>>();
        let prior_resolutions = self
            .store
            .list_ticket_resolutions(&ticket.id)?
            .into_iter()
            .filter_map(|resolution| fs::read_to_string(&resolution.resolution_path).ok())
            .map(|text| self.redact_text(&text))
            .collect::<Vec<_>>();
        let current_diff = task
            .worktree_path
            .as_ref()
            .and_then(|path| self.workspace.capture_diff(path, &run_id).ok())
            .map(|diff| self.redact_text(&diff));
        let mut sections = vec![
            ContextSection {
                label: "objective".to_string(),
                priority: 0,
                budget_class: ContextBudgetClass::Objective,
                required: true,
                body: format!(
                    "prompt: {}\nsummary: {}\nstatus: {}",
                    self.redact_text(&objective.prompt),
                    self.redact_text(&objective.summary),
                    objective.status.as_str()
                ),
            },
            ContextSection {
                label: "task".to_string(),
                priority: 10,
                budget_class: ContextBudgetClass::State,
                required: true,
                body: format!(
                    "title: {}\ngoal: {}\nstatus: {}",
                    self.redact_text(&task.title),
                    self.redact_text(&task.goal),
                    task.status.as_str()
                ),
            },
            ContextSection {
                label: "ticket".to_string(),
                priority: 20,
                budget_class: ContextBudgetClass::State,
                required: true,
                body: format!(
                    "blocked_on: {}\nreason: {}\nquestion: {}\nevidence: {}",
                    self.redact_text(&ticket.blocked_on),
                    self.redact_text(&ticket.reason),
                    self.redact_text(&ticket.question),
                    self.redact_text(&ticket.evidence_json)
                ),
            },
        ];
        if let Some(diff) = current_diff {
            sections.push(ContextSection {
                label: "current_diff".to_string(),
                priority: 30,
                budget_class: ContextBudgetClass::ArtifactExcerpt,
                required: false,
                body: diff,
            });
        }
        let context = pack_context(ContextPackRequest {
            budget: ContextBudget {
                total_bytes: 32 * 1024,
                objective_bytes: 8 * 1024,
                conversation_bytes: 2 * 1024,
                state_bytes: 10 * 1024,
                artifact_excerpt_bytes: 8 * 1024,
                schema_bytes: 4 * 1024,
            },
            sections,
            artifacts: Vec::new(),
        })?;
        let request = build_ticket_resolver_request(
            self.redact_text(&objective.prompt),
            self.redact_text(&objective.summary),
            criteria,
            TicketResolverStatuses {
                objective_status: objective.status.as_str().to_string(),
                task_status: task.status.as_str().to_string(),
                ticket_status: ticket.status.as_str().to_string(),
            },
            TicketResolverTicketDetails {
                blocked_on: self.redact_text(&ticket.blocked_on),
                reason: self.redact_text(&ticket.reason),
                question: self.redact_text(&ticket.question),
            },
            TicketResolverTaskDetails {
                title: self.redact_text(&task.title),
                goal: self.redact_text(&task.goal),
                validation_commands: validations,
            },
            prior_resolutions,
            context.manifest,
        );
        let prompt = build_ticket_resolver_prompt(request)?;
        let request_artifact = self.write_objective_artifact(
            objective_id,
            objective.active_plan_id.as_ref(),
            Some(&exchange_id),
            "ticket_resolver_request",
            &format!("resolver-{}-request.json", ticket.id.as_str()),
            &prompt.input,
        )?;
        self.store
            .insert_objective_artifact(request_artifact.clone())?;
        let response = match super::block_on(self.openai.complete(ModelRequest {
            model: model.clone(),
            system: prompt.system,
            input: prompt.input,
            temperature: Some(0.0),
            max_output_tokens: Some(self.config.providers.openai.max_output_tokens),
            metadata: BTreeMap::from([
                ("role".to_string(), "objective_ticket_resolver".to_string()),
                (
                    "objective_id".to_string(),
                    objective_id.as_str().to_string(),
                ),
                ("ticket_id".to_string(), ticket.id.as_str().to_string()),
            ]),
        })) {
            Ok(response) => response,
            Err(error) => {
                let error = super::provider_error(error);
                self.persist_resolver_exchange(
                    objective_id,
                    &exchange_id,
                    &ticket.id,
                    &model,
                    Some(&request_artifact),
                    None,
                    "failed",
                    Some(&error),
                )?;
                self.fail_resolver_attempt_and_ticket(
                    attempt_id,
                    resolver_owner,
                    &exchange_id,
                    &ticket.id,
                    task_owner,
                    &error,
                )?;
                return Err(HarnessError::External(error));
            }
        };
        let response_artifact = self.write_objective_artifact(
            objective_id,
            objective.active_plan_id.as_ref(),
            Some(&exchange_id),
            "ticket_resolver_response",
            &format!("resolver-{}-response.json", ticket.id.as_str()),
            &response.text,
        )?;
        self.store
            .insert_objective_artifact(response_artifact.clone())?;
        let parsed = match parse_ticket_resolver_response(&response.text) {
            Ok(parsed) => parsed,
            Err(error) => {
                let error_text = error.to_string();
                self.persist_resolver_exchange(
                    objective_id,
                    &exchange_id,
                    &ticket.id,
                    &model,
                    Some(&request_artifact),
                    Some(&response_artifact),
                    "rejected",
                    Some(&error_text),
                )?;
                self.fail_resolver_attempt_and_ticket(
                    attempt_id,
                    resolver_owner,
                    &exchange_id,
                    &ticket.id,
                    task_owner,
                    &error_text,
                )?;
                return Err(error);
            }
        };

        let guidance = add_deterministic_unblock_guidance(
            render_resolver_guidance(&parsed),
            &objective,
            &task,
            &ticket,
        );
        let resolution_path = self.write_artifact(
            &task.id,
            Some(&run_id),
            None,
            Some(&ticket.id),
            "objective-resolution.md",
            &guidance,
        )?;
        let resolution = TicketResolution {
            id: super::new_id(TicketResolutionId::PREFIX, TicketResolutionId::parse)?,
            ticket_id: ticket.id.clone(),
            provider: response.provider,
            model: response.model,
            response_id: response.response_id,
            resolution_path: resolution_path.clone(),
            consumed_at: None,
            created_at: crate::orchestrator::orchestrator_now(),
        };
        self.store
            .insert_ticket_resolution(resolution.clone(), task_owner)?;
        self.insert_artifact(
            &task.id,
            Some(&run_id),
            None,
            Some(&ticket.id),
            "ticket_resolution",
            resolution_path,
            task_owner,
        )?;
        self.persist_resolver_exchange(
            objective_id,
            &exchange_id,
            &ticket.id,
            &model,
            Some(&request_artifact),
            Some(&response_artifact),
            "accepted",
            None,
        )?;
        self.store.release_resolver_attempt_lease(
            attempt_id,
            resolver_owner,
            "resolved",
            Some(&exchange_id),
            None,
        )?;
        let resolved_message = format!(
            "resolved ticket {}: {}",
            ticket.id,
            resolution_excerpt(&guidance)
        );
        self.persist_objective_event(
            objective_id,
            ObjectiveProgressKind::TicketResolutionCompleted,
            resolved_message.clone(),
            json!({
                "ticket_id": ticket.id.as_str(),
                "task_id": task.id.as_str(),
                "resolution_id": resolution.id.as_str(),
                "planner_exchange_id": exchange_id.as_str(),
                "resolution_excerpt": resolution_excerpt(&guidance),
            }),
        )?;
        Ok(vec![task_ticket_event(
            ObjectiveProgressKind::TicketResolutionCompleted,
            objective_id,
            &task.id,
            &ticket.id,
            ObjectiveProgressPhase::Running,
            ObjectiveStatus::Running,
            resolved_message,
        )])
    }

    fn persist_resolver_exchange(
        &self,
        objective_id: &ObjectiveId,
        exchange_id: &PlannerExchangeId,
        ticket_id: &TicketId,
        model: &str,
        request_artifact: Option<&ObjectiveArtifact>,
        response_artifact: Option<&ObjectiveArtifact>,
        status: &str,
        error: Option<&str>,
    ) -> HarnessResult<()> {
        self.store.insert_planner_exchange(PlannerExchange {
            id: exchange_id.clone(),
            objective_id: objective_id.clone(),
            kind: PlannerExchangeKind::TicketResolution,
            ticket_id: Some(ticket_id.clone()),
            model: model.to_string(),
            system_prompt_version: crate::prompts::PROMPT_CONTRACT_VERSION.to_string(),
            request_objective_artifact_id: request_artifact.map(|artifact| artifact.id.clone()),
            response_objective_artifact_id: response_artifact.map(|artifact| artifact.id.clone()),
            status: status.to_string(),
            error: error.map(ToOwned::to_owned),
            created_at: crate::orchestrator::orchestrator_now(),
        })
    }

    fn fail_resolver_attempt_and_ticket(
        &self,
        attempt_id: &ObjectiveResolverAttemptId,
        resolver_owner: &str,
        exchange_id: &PlannerExchangeId,
        ticket_id: &TicketId,
        task_owner: &str,
        error: &str,
    ) -> HarnessResult<()> {
        let _ = self.store.release_resolver_attempt_lease(
            attempt_id,
            resolver_owner,
            "failed",
            Some(exchange_id),
            Some(error),
        );
        let ticket = self.store.get_ticket(ticket_id)?;
        if ticket.status == TicketStatus::Resolving {
            self.store.transition_ticket(
                ticket_id,
                TicketStatus::Resolving,
                TicketStatus::Failed,
                task_owner,
            )?;
        }
        Ok(())
    }

    fn persist_objective_event(
        &self,
        objective_id: &ObjectiveId,
        kind: ObjectiveProgressKind,
        message: String,
        payload: serde_json::Value,
    ) -> HarnessResult<()> {
        self.store.insert_objective_event(ObjectiveEvent {
            id: super::new_id(ObjectiveEventId::PREFIX, ObjectiveEventId::parse)?,
            objective_id: objective_id.clone(),
            event_type: kind.as_str().to_string(),
            message,
            payload_json: payload.to_string(),
            created_at: crate::orchestrator::orchestrator_now(),
        })
    }

    fn block_objective(
        &self,
        objective_id: &ObjectiveId,
        message: String,
    ) -> HarnessResult<ObjectiveCycleResult> {
        self.store.update_objective_status(
            objective_id,
            None,
            ObjectiveStatusUpdate {
                status: Some(ObjectiveStatus::Blocked),
                summary: Some(message.clone()),
                updated_at: Some(crate::orchestrator::orchestrator_now()),
                ..ObjectiveStatusUpdate::default()
            },
        )?;
        Ok(ObjectiveCycleResult::Terminal(
            CommandResult::with_data(
                CommandExit::failure(message.clone()),
                terminal_data(objective_id, ObjectiveStatus::Blocked, "blocked"),
            )
            .with_event(event(
                ObjectiveProgressKind::Blocked,
                objective_id,
                ObjectiveProgressPhase::Blocked,
                ObjectiveStatus::Blocked,
                message,
            )),
        ))
    }

    fn fail_objective(
        &self,
        objective_id: &ObjectiveId,
        message: String,
        exit: CommandExit,
    ) -> HarnessResult<ObjectiveCycleResult> {
        self.store.update_objective_status(
            objective_id,
            None,
            ObjectiveStatusUpdate {
                status: Some(ObjectiveStatus::Failed),
                summary: Some(message.clone()),
                updated_at: Some(crate::orchestrator::orchestrator_now()),
                ..ObjectiveStatusUpdate::default()
            },
        )?;
        Ok(ObjectiveCycleResult::Terminal(
            CommandResult::with_data(
                exit,
                terminal_data(objective_id, ObjectiveStatus::Failed, "failed"),
            )
            .with_event(event(
                ObjectiveProgressKind::Failed,
                objective_id,
                ObjectiveProgressPhase::Failed,
                ObjectiveStatus::Failed,
                message,
            )),
        ))
    }

    fn cancel_objective_monitor(&self, objective_id: &ObjectiveId) -> HarnessResult<CommandResult> {
        self.store.update_objective_status(
            objective_id,
            None,
            ObjectiveStatusUpdate {
                status: Some(ObjectiveStatus::Cancelled),
                summary: Some("objective supervision cancelled".to_string()),
                updated_at: Some(crate::orchestrator::orchestrator_now()),
                ..ObjectiveStatusUpdate::default()
            },
        )?;
        Ok(CommandResult::with_data(
            CommandExit::failure("objective supervision cancelled"),
            terminal_data(objective_id, ObjectiveStatus::Cancelled, "cancelled"),
        )
        .with_event(event(
            ObjectiveProgressKind::Cancelled,
            objective_id,
            ObjectiveProgressPhase::Cancelled,
            ObjectiveStatus::Cancelled,
            "objective supervision cancelled",
        )))
    }
}

#[derive(Debug)]
pub enum ObjectiveCycleResult {
    Progress(Vec<CommandEvent>),
    Terminal(CommandResult),
}

fn objective_owner(objective_id: &ObjectiveId) -> String {
    format!("objective-monitor-{}-{}", std::process::id(), objective_id)
}

fn terminal_data(
    objective_id: &ObjectiveId,
    status: ObjectiveStatus,
    reason: &str,
) -> serde_json::Value {
    json!({
        "objective_id": objective_id.as_str(),
        "status": status.as_str(),
        "terminal": true,
        "exit_reason": reason,
        "task_ids": [],
        "open_ticket_ids": [],
        "validation": {
            "status": "skipped",
            "commands_run": 0,
            "commands_skipped": 0,
        },
        "next": format!("harness objective get {}", objective_id.as_str()),
    })
}

fn limit_worker_attempts(option_limit: Option<u32>, remaining: u32) -> u32 {
    option_limit.unwrap_or(remaining).min(remaining).max(1)
}

#[derive(Debug)]
struct ObjectiveValidationResult {
    passed: bool,
    commands_run: u32,
    failing_command: Option<String>,
    failure_log_path: Option<String>,
}

fn validation_log(command: &str, output: &CommandOutput) -> String {
    format!(
        "$ {command}\nexit: {:?}\ntimed_out: {}\nstdout:\n{}\nstderr:\n{}",
        output.exit_code, output.timed_out, output.stdout, output.stderr
    )
}

fn unique_validation_failure_artifact_name() -> HarnessResult<String> {
    let id = super::new_id(ObjectiveArtifactId::PREFIX, ObjectiveArtifactId::parse)?;
    Ok(format!("objective-validation-failure-{}.log", id.as_str()))
}

fn event(
    kind: ObjectiveProgressKind,
    objective_id: &ObjectiveId,
    phase: ObjectiveProgressPhase,
    status: ObjectiveStatus,
    message: impl Into<String>,
) -> CommandEvent {
    let mut progress = crate::runtime::ObjectiveProgressEvent::new(
        kind,
        objective_id.clone(),
        phase,
        status,
        message,
        crate::orchestrator::orchestrator_now(),
    );
    progress.next_command = Some(format!(
        "harness objective get {} --output json",
        objective_id
    ));
    CommandEvent::objective_progress(progress, CommandEventLevel::Info)
}

fn task_event(
    kind: ObjectiveProgressKind,
    objective_id: &ObjectiveId,
    task_id: &TaskId,
    phase: ObjectiveProgressPhase,
    status: ObjectiveStatus,
    message: impl Into<String>,
) -> CommandEvent {
    let mut event = event(kind, objective_id, phase, status, message);
    if let Some(progress) = &mut event.objective_progress {
        progress.task_id = Some(task_id.clone());
    }
    event
}

fn task_ticket_event(
    kind: ObjectiveProgressKind,
    objective_id: &ObjectiveId,
    task_id: &TaskId,
    ticket_id: &TicketId,
    phase: ObjectiveProgressPhase,
    status: ObjectiveStatus,
    message: impl Into<String>,
) -> CommandEvent {
    let mut event = task_event(kind, objective_id, task_id, phase, status, message);
    if let Some(progress) = &mut event.objective_progress {
        progress.ticket_id = Some(ticket_id.clone());
    }
    event
}

fn ticket_detected_message(task_id: &TaskId, ticket: &crate::domain::Ticket) -> String {
    format!(
        "detected ticket {} for generated task {}: {}",
        ticket.id,
        task_id,
        ticket_summary(ticket)
    )
}

fn ticket_summary(ticket: &crate::domain::Ticket) -> String {
    let reason = compact_one_line(&ticket.reason, 120);
    let question = compact_one_line(&ticket.question, 100);
    if question.is_empty() {
        format!("{}: {}", ticket.blocked_on, reason)
    } else {
        format!("{}: {}; question: {}", ticket.blocked_on, reason, question)
    }
}

fn resolution_excerpt(text: &str) -> String {
    let mut capture_next = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("## Diagnosis") {
            capture_next = true;
            continue;
        }
        if capture_next && !trimmed.is_empty() && !trimmed.starts_with('#') {
            return compact_one_line(trimmed, 180);
        }
    }
    compact_one_line(
        text.lines()
            .find(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty() && !trimmed.starts_with('#')
            })
            .unwrap_or("resolver produced guidance"),
        180,
    )
}

fn compact_one_line(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut output = compact
        .chars()
        .take(max_chars.saturating_sub(12))
        .collect::<String>();
    output.push_str(" [truncated]");
    output
}

fn stream_new_events(
    sink: &mut Option<&mut dyn OutputSink>,
    events: &[CommandEvent],
    skip: usize,
) -> HarnessResult<()> {
    let Some(sink) = sink.as_deref_mut() else {
        return Ok(());
    };
    for event in events.iter().skip(skip) {
        sink.event(event)?;
    }
    Ok(())
}

fn starts_with_worker_started(events: &[CommandEvent]) -> bool {
    events
        .first()
        .and_then(|event| event.objective_progress.as_ref())
        .is_some_and(|progress| progress.kind == ObjectiveProgressKind::WorkerStarted)
}

fn worker_completed_message(
    task: &Task,
    result: &CommandResult,
    store: &dyn TaskStore,
) -> HarnessResult<String> {
    let output = latest_local_worker_output(result);
    let Some(run_id) = result
        .data
        .get("run_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            result
                .data
                .get("run_ids")
                .and_then(serde_json::Value::as_array)
                .and_then(|run_ids| run_ids.iter().rev().find_map(serde_json::Value::as_str))
        })
        .and_then(|run_id| RunId::parse(run_id).ok())
    else {
        return Ok(match output {
            Some(output) => format!("generated task {} completed:\n{}", task.id, output),
            None => format!("generated task {} completed", task.id),
        });
    };
    let attempts = store.list_attempts(&run_id)?;
    let Some(attempt) = attempts
        .iter()
        .rev()
        .find(|attempt| attempt.provider == "ollama")
    else {
        return Ok(format!("generated task {} completed", task.id));
    };
    if let Some(output) = output {
        return Ok(format!(
            "generated task {} completed via local Ollama model {} attempt {} ({}):\n{}",
            task.id, attempt.model, attempt.attempt_number, attempt.status, output
        ));
    }
    Ok(format!(
        "generated task {} completed via local Ollama model {} attempt {} ({})",
        task.id, attempt.model, attempt.attempt_number, attempt.status
    ))
}

fn latest_local_worker_output(result: &CommandResult) -> Option<String> {
    result
        .data
        .get("progress_events")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .rev()
        .filter_map(|event| event.get("message").and_then(serde_json::Value::as_str))
        .find_map(|message| {
            message
                .find("local Ollama ")
                .map(|index| message[index..].to_string())
        })
}

fn render_resolver_guidance(response: &TicketResolverResponse) -> String {
    let mut text = String::new();
    text.push_str("# Objective Ticket Resolution\n\n");
    text.push_str("## Diagnosis\n\n");
    text.push_str(response.diagnosis.trim());
    text.push_str("\n\n## Recommended Steps\n\n");
    for step in &response.recommended_steps {
        text.push_str("- ");
        text.push_str(step.trim());
        text.push('\n');
    }
    if !response.constraints.is_empty() {
        text.push_str("\n## Constraints\n\n");
        for constraint in &response.constraints {
            text.push_str("- ");
            text.push_str(constraint.trim());
            text.push('\n');
        }
    }
    if !response.validation_focus.is_empty() {
        text.push_str("\n## Validation Focus\n\n");
        for item in &response.validation_focus {
            text.push_str("- ");
            text.push_str(item.trim());
            text.push('\n');
        }
    }
    text
}

fn add_deterministic_unblock_guidance(
    mut guidance: String,
    objective: &crate::state::Objective,
    task: &Task,
    ticket: &crate::domain::Ticket,
) -> String {
    guidance.push_str("\n## Supervisor Execution Guidance\n\n");
    guidance.push_str("- Treat the objective prompt, task goal, acceptance criteria, and validation commands as enough authority to choose reasonable conventional defaults.\n");
    guidance.push_str("- Do not ask the same broad product-scope question again after this resolution. If details are underspecified, implement the smallest conventional version that satisfies the task and validation.\n");
    guidance.push_str("- If expected files or scaffolding are absent, create the minimal conventional structure needed for the requested implementation.\n");
    guidance.push_str("- Return a concrete unified git diff unless a specific missing fact cannot be reasonably defaulted and directly blocks validation.\n");

    if rust_scaffold_blocker(objective, task, ticket) {
        guidance.push_str("\n## Supervisor Deterministic Guidance\n\n");
        guidance.push_str(
            "- This repository may legitimately be empty. Missing Cargo.toml or src/main.rs is not a blocker.\n",
        );
        guidance.push_str(
            "- Do not return STUCK because Rust source files or crate structure are missing. Create the scaffold in the next diff.\n",
        );
        guidance.push_str(
            "- Create Cargo.toml with a standard [package] section, edition = \"2021\", and no [bin] table unless explicitly needed.\n",
        );
        guidance.push_str(
            "- Create src/main.rs and implement the requested Rust application there. For a simple terminal game, prefer std-only code unless the existing project already has dependencies.\n",
        );
        guidance.push_str(
            "- Return a unified git diff that creates Cargo.toml and src/main.rs using --- /dev/null and +++ b/<path> hunks.\n",
        );
    }
    guidance
}

fn rust_scaffold_blocker(
    objective: &crate::state::Objective,
    task: &Task,
    ticket: &crate::domain::Ticket,
) -> bool {
    let intent = format!("{} {} {}", objective.prompt, task.title, task.goal).to_ascii_lowercase();
    if !intent.contains("rust") && !intent.contains("cargo") {
        return false;
    }
    let blocker = format!(
        "{} {} {}",
        ticket.blocked_on, ticket.reason, ticket.question
    )
    .to_ascii_lowercase();
    let mentions_missing_source = blocker.contains("missing")
        || blocker.contains("no rust source")
        || blocker.contains("no source")
        || blocker.contains("not found");
    let mentions_scaffold = blocker.contains("cargo.toml")
        || blocker.contains("src/main.rs")
        || blocker.contains("crate")
        || blocker.contains("source files")
        || blocker.contains("repository context")
        || blocker.contains("project structure");
    mentions_missing_source && mentions_scaffold
}

struct NeverCancelled;

impl CancellationToken for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}
