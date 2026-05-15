pub mod objective;
pub mod supervisor;
pub mod supervisor_state;

use crate::domain::{
    Artifact, ArtifactId, Attempt, AttemptId, AttemptStatus, Event, EventId, EventLevel,
    HarnessConfig, ObjectiveAcceptanceCriterionId, ObjectiveAcceptanceStatus, ObjectiveArtifactId,
    ObjectiveEventId, ObjectiveId, ObjectiveMessageId, ObjectivePlanId, ObjectiveStatus,
    ObjectiveValidationCommandId, ObjectiveValidationCommandSource, PlannerExchangeId,
    PlannerExchangeKind, Run, RunId, RunStatus, Task, TaskId, TaskStatus, Ticket, TicketId,
    TicketResolution, TicketResolutionId, TicketStatus,
};
use crate::patch::{OllamaResponse, parse_ollama_response};
use crate::planner::{
    ContextBudget, ContextBudgetClass, ContextPackRequest, ContextSection, PlannerResponse,
    build_planner_request, pack_context, parse_planner_response_for_repo,
};
use crate::prompts::{
    ArtifactManifestContext, OllamaPromptRequest, TicketPromptRequest,
    build_artifact_manifest_with_context,
};
use crate::prompts::{build_ollama_worker_prompt, build_planner_prompt, build_ticket_prompt};
use crate::providers::{ModelProvider, ModelRequest, ModelResponse, ProviderError};
use crate::runtime::{
    CommandEvent, CommandEventLevel, CommandExit, CommandResult, CommandStatus,
    ObjectiveProgressKind, ObjectiveProgressPhase, ObjectivePromptInput, ObjectiveStartOptions,
    OutputSink, ResumeTaskOptions, TaskRunOptions, TicketResolveOptions,
};
use crate::security::{DefaultRedactor, Redactor, ValidationCommandPolicy};
use crate::state::{
    NewGeneratedTask, NewObjectiveMessage, NewObjectiveTaskDependency, Objective,
    ObjectiveAcceptanceCriterion, ObjectiveArtifact, ObjectiveEvent, ObjectivePlan,
    ObjectivePlanBundle, ObjectiveStatusUpdate, ObjectiveTaskValidationCommand,
    ObjectiveValidationCommand, PlannerExchange, RejectedPlannerExchange, RunUpdate, TaskStore,
};
use crate::workspace::{
    CommandRunner, CommandSpec, CommandStdin, PatchCheck, RecordedWorktree, WorkspaceManager,
    WorktreeRequest,
};
use crate::{HarnessError, HarnessResult};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct RunOrchestrator {
    config: HarnessConfig,
    store: Arc<dyn TaskStore>,
    workspace: Arc<dyn WorkspaceManager>,
    runner: Arc<dyn CommandRunner>,
    ollama: Arc<dyn ModelProvider>,
    openai: Arc<dyn ModelProvider>,
    redactor: DefaultRedactor,
}

#[derive(Debug, Clone)]
struct AttemptContext {
    attempt_count: u32,
    validation_log: Option<String>,
    validation_log_path: Option<String>,
    validation_log_hash: Option<String>,
    validation_exit_code: Option<i32>,
    last_validation_command: Option<String>,
    last_validation_cwd: Option<String>,
    summaries: Vec<String>,
    last_response: Option<String>,
    last_response_path: Option<String>,
    last_response_hash: Option<String>,
}

#[derive(Debug, Clone)]
struct ResumeResolution {
    resolution: TicketResolution,
    text: String,
}

#[derive(Debug, Clone)]
struct ResponseOutcome {
    status: AttemptStatus,
    reason: Option<String>,
    validation_log: Option<String>,
    validation_exit_code: Option<i32>,
    validation_command: Option<String>,
    validation_cwd: Option<String>,
    apply_error: Option<String>,
    patch_path: Option<String>,
    apply_check_stderr_path: Option<String>,
    apply_stderr_path: Option<String>,
}

impl RunOrchestrator {
    pub fn new(
        config: HarnessConfig,
        store: Arc<dyn TaskStore>,
        workspace: Arc<dyn WorkspaceManager>,
        runner: Arc<dyn CommandRunner>,
        ollama: Arc<dyn ModelProvider>,
        openai: Arc<dyn ModelProvider>,
    ) -> Self {
        Self {
            config,
            store,
            workspace,
            runner,
            ollama,
            openai,
            redactor: DefaultRedactor::new(),
        }
    }

    pub fn create_task(
        &self,
        title: String,
        goal: String,
        validation_commands: Vec<String>,
    ) -> HarnessResult<Task> {
        if title.trim().is_empty() || goal.trim().is_empty() {
            return Err(HarnessError::Usage(
                "task title and goal cannot be empty".to_string(),
            ));
        }
        if validation_commands.is_empty()
            || validation_commands.iter().any(|cmd| cmd.trim().is_empty())
        {
            return Err(HarnessError::Usage(
                "task create requires at least one non-empty validation command".to_string(),
            ));
        }
        let now = now();
        let title = self.redact_text(&title);
        let goal = self.redact_text(&goal);
        let validation_commands = validation_commands
            .into_iter()
            .map(|command| self.redact_text(&command))
            .collect();
        let task = Task {
            id: new_id(TaskId::PREFIX, TaskId::parse)?,
            title,
            goal,
            status: TaskStatus::Ready,
            repo_root: self.workspace.discover_repo_root(None)?,
            worktree_path: None,
            branch: None,
            base_ref: Some("HEAD".to_string()),
            base_commit: None,
            last_seen_head: None,
            max_attempts: self.config.orchestrator.max_attempts,
            lease_owner: None,
            lease_acquired_at: None,
            lease_expires_at: None,
            heartbeat_at: None,
            lock_version: 0,
            created_at: now.clone(),
            updated_at: now,
        };
        self.store.insert_task(task.clone(), validation_commands)?;
        Ok(task)
    }

    pub fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
        self.store.list_tasks(None)
    }

    pub fn list_objectives(
        &self,
        status: Option<ObjectiveStatus>,
    ) -> HarnessResult<Vec<Objective>> {
        self.store.list_objectives(status)
    }

    pub fn get_objective(&self, objective_id: &ObjectiveId) -> HarnessResult<Objective> {
        self.store.get_objective(objective_id)
    }

    pub fn get_objective_plan(&self, objective_id: &ObjectiveId) -> HarnessResult<CommandResult> {
        let objective = self.store.get_objective(objective_id)?;
        let exchanges = self.store.list_planner_exchanges(objective_id)?;
        let artifacts = self.store.list_objective_artifacts(objective_id)?;
        let messages = self.store.list_objective_messages(objective_id)?;
        let events = self.store.list_objective_events(objective_id)?;
        Ok(CommandResult::with_data(
            CommandExit::success(),
            json!({
                "objective_id": objective.id.as_str(),
                "active_plan_id": objective.active_plan_id.as_ref().map(ObjectivePlanId::as_str),
                "summary": objective.summary,
                "planner_exchanges": exchanges.iter().map(|exchange| json!({
                    "id": exchange.id.as_str(),
                    "kind": exchange.kind.as_str(),
                    "ticket_id": exchange.ticket_id.as_ref().map(TicketId::as_str),
                    "model": exchange.model,
                    "status": exchange.status,
                    "error": exchange.error,
                    "created_at": exchange.created_at,
                })).collect::<Vec<_>>(),
                "artifacts": artifacts.iter().map(|artifact| json!({
                    "id": artifact.id.as_str(),
                    "kind": artifact.kind,
                    "path": artifact.path,
                    "sha256": artifact.sha256,
                    "byte_len": artifact.byte_len,
                    "created_at": artifact.created_at,
                })).collect::<Vec<_>>(),
                "messages": messages.iter().map(|message| json!({
                    "id": message.id.as_str(),
                    "sequence": message.sequence,
                    "role": message.role,
                    "kind": message.kind,
                    "content_preview": message.content_preview,
                    "created_at": message.created_at,
                })).collect::<Vec<_>>(),
                "events": events.iter().map(|event| json!({
                    "id": event.id.as_str(),
                    "event_type": event.event_type,
                    "message": event.message,
                    "payload_json": event.payload_json,
                    "created_at": event.created_at,
                })).collect::<Vec<_>>(),
                "next": format!("harness objective supervise {}", objective.id.as_str()),
            }),
        ))
    }

    pub fn start_objective(&self, options: ObjectiveStartOptions) -> HarnessResult<CommandResult> {
        self.start_objective_inner(options, None)
    }

    pub fn start_objective_streaming(
        &self,
        options: ObjectiveStartOptions,
        sink: &mut dyn OutputSink,
    ) -> HarnessResult<CommandResult> {
        self.start_objective_inner(options, Some(sink))
    }

    fn start_objective_inner(
        &self,
        options: ObjectiveStartOptions,
        mut sink: Option<&mut dyn OutputSink>,
    ) -> HarnessResult<CommandResult> {
        let prompt = self.resolve_objective_prompt(&options.prompt)?;
        if prompt.trim().is_empty() {
            return Err(HarnessError::Usage(
                "objective prompt cannot be empty".to_string(),
            ));
        }
        let repo_root = self.workspace.discover_repo_root(
            options
                .runtime
                .repo
                .as_deref()
                .and_then(|path| path.to_str()),
        )?;
        let created_at = now();
        let objective_id = new_id(ObjectiveId::PREFIX, ObjectiveId::parse)?;
        let objective = Objective {
            id: objective_id.clone(),
            title: title_from_prompt(&prompt),
            prompt: self.redact_text(&prompt),
            summary: "Planning objective".to_string(),
            status: ObjectiveStatus::Planning,
            planner_model: Some(
                options
                    .planner_model
                    .clone()
                    .unwrap_or_else(|| self.config.providers.openai.default_model.clone()),
            ),
            worker_model: Some(
                options
                    .worker_model
                    .clone()
                    .unwrap_or_else(|| self.config.providers.ollama.default_model.clone()),
            ),
            ticket_model: options.ticket_model.clone(),
            active_plan_id: None,
            monitor_lease_owner: None,
            monitor_lease_expires_at: None,
            created_at: created_at.clone(),
            updated_at: created_at.clone(),
        };
        self.store.insert_objective(objective.clone())?;

        let planning_started = objective_progress(
            ObjectiveProgressKind::PlanningStarted,
            &objective_id,
            ObjectiveProgressPhase::Planning,
            ObjectiveStatus::Planning,
            "planning objective",
        );
        if let Some(sink) = sink.as_deref_mut() {
            sink.event(&CommandEvent::objective_progress(
                planning_started.clone(),
                CommandEventLevel::Info,
            ))?;
        }

        let context = pack_context(ContextPackRequest {
            budget: ContextBudget {
                total_bytes: 32 * 1024,
                objective_bytes: 8 * 1024,
                conversation_bytes: 4 * 1024,
                state_bytes: 8 * 1024,
                artifact_excerpt_bytes: 4 * 1024,
                schema_bytes: 8 * 1024,
            },
            sections: vec![
                ContextSection {
                    label: "objective_prompt".to_string(),
                    priority: 0,
                    budget_class: ContextBudgetClass::Objective,
                    required: true,
                    body: self.redact_text(&prompt),
                },
                ContextSection {
                    label: "repository".to_string(),
                    priority: 10,
                    budget_class: ContextBudgetClass::State,
                    required: false,
                    body: format!("repo_root: {repo_root}"),
                },
            ],
            artifacts: Vec::new(),
        })?;
        let planner_request = build_planner_request(
            self.redact_text(&prompt),
            context.body.clone(),
            context.manifest,
        );
        let planner_prompt = build_planner_prompt(planner_request)?;
        let request_text = planner_prompt.input.clone();
        let planner_model = objective.planner_model.clone().unwrap_or_default();
        let exchange_id = new_id(PlannerExchangeId::PREFIX, PlannerExchangeId::parse)?;
        let request_artifact = self.write_objective_artifact(
            &objective_id,
            None,
            Some(&exchange_id),
            "planner_request",
            "planner-request.json",
            &request_text,
        )?;
        let response = match block_on(self.openai.complete(ModelRequest {
            model: planner_model.clone(),
            system: planner_prompt.system,
            input: planner_prompt.input,
            temperature: Some(0.0),
            max_output_tokens: Some(self.config.providers.openai.max_output_tokens),
            metadata: BTreeMap::from([
                ("role".to_string(), "objective_planner".to_string()),
                (
                    "objective_id".to_string(),
                    objective_id.as_str().to_string(),
                ),
            ]),
        })) {
            Ok(response) => response,
            Err(error) => {
                let error = HarnessError::External(provider_error(error));
                self.persist_failed_planner_call(
                    &objective_id,
                    exchange_id,
                    planner_model,
                    request_artifact,
                    error.to_string(),
                )?;
                let result = objective_failure_result(
                    &objective_id,
                    planning_started,
                    ObjectiveProgressKind::Failed,
                    ObjectiveProgressPhase::Failed,
                    ObjectiveStatus::Failed,
                    error,
                );
                stream_objective_start_result(&mut sink, &result)?;
                return Ok(result);
            }
        };

        let plan_id = new_id(ObjectivePlanId::PREFIX, ObjectivePlanId::parse)?;
        let response_artifact = self.write_objective_artifact(
            &objective_id,
            None,
            Some(&exchange_id),
            "planner_response",
            "planner-response.json",
            &response.text,
        )?;

        let parsed = match parse_planner_response_for_repo(&response.text, &repo_root) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.persist_rejected_plan(
                    &objective_id,
                    exchange_id,
                    planner_model,
                    request_artifact,
                    response_artifact,
                    error.to_string(),
                )?;
                let result = objective_failure_result(
                    &objective_id,
                    planning_started,
                    ObjectiveProgressKind::PlanRejected,
                    ObjectiveProgressPhase::Failed,
                    ObjectiveStatus::Failed,
                    error,
                );
                stream_objective_start_result(&mut sink, &result)?;
                return Ok(result);
            }
        };

        let bundle = match self.build_plan_bundle(
            &objective,
            plan_id,
            exchange_id.clone(),
            planner_model.clone(),
            parsed,
            request_artifact.clone(),
            response_artifact.clone(),
            &repo_root,
            options
                .max_worker_attempts
                .unwrap_or(self.config.orchestrator.max_attempts),
        ) {
            Ok(bundle) => bundle,
            Err(error) => {
                self.persist_rejected_plan(
                    &objective_id,
                    exchange_id,
                    planner_model,
                    request_artifact,
                    response_artifact,
                    error.to_string(),
                )?;
                let result = objective_failure_result(
                    &objective_id,
                    planning_started,
                    ObjectiveProgressKind::PlanRejected,
                    ObjectiveProgressPhase::Failed,
                    ObjectiveStatus::Failed,
                    error,
                );
                stream_objective_start_result(&mut sink, &result)?;
                return Ok(result);
            }
        };
        let bundle_result = self
            .store
            .create_objective_plan_bundle(&objective_id, bundle)?;
        let task_ids = bundle_result
            .tasks
            .iter()
            .map(|task| task.task_id.as_str().to_string())
            .collect::<Vec<_>>();
        let result = CommandResult::with_data(
            CommandExit::success(),
            json!({
                "objective_id": objective_id.as_str(),
                "plan_id": bundle_result.plan.id.as_str(),
                "status": bundle_result.objective.status.as_str(),
                "terminal": false,
                "task_ids": task_ids,
                "open_ticket_ids": [],
                "validation": {
                    "status": "pending",
                    "commands_run": 0,
                    "commands_skipped": 0,
                },
                "next": format!("harness objective supervise {}", objective_id.as_str()),
            }),
        )
        .with_event(CommandEvent::objective_progress(
            planning_started,
            CommandEventLevel::Info,
        ))
        .with_event(CommandEvent::objective_progress(
            objective_progress(
                ObjectiveProgressKind::PlanningCompleted,
                &objective_id,
                ObjectiveProgressPhase::Ready,
                ObjectiveStatus::Ready,
                "objective plan accepted",
            ),
            CommandEventLevel::Info,
        ));
        stream_objective_start_result(&mut sink, &result)?;
        Ok(result)
    }

    pub fn get_task(&self, task_id: &TaskId) -> HarnessResult<Task> {
        self.store.get_task(task_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_plan_bundle(
        &self,
        objective: &Objective,
        plan_id: ObjectivePlanId,
        exchange_id: PlannerExchangeId,
        planner_model: String,
        response: PlannerResponse,
        request_artifact: ObjectiveArtifact,
        response_artifact: ObjectiveArtifact,
        repo_root: &str,
        worker_attempt_budget: u32,
    ) -> HarnessResult<ObjectivePlanBundle> {
        let policy = ValidationCommandPolicy::new();
        let created_at = now();
        let plan = ObjectivePlan {
            id: plan_id.clone(),
            objective_id: objective.id.clone(),
            version: 1,
            summary: response.objective.summary.clone(),
            created_at: created_at.clone(),
        };
        let acceptance_criteria = response
            .objective
            .acceptance_criteria
            .iter()
            .map(|criterion| {
                Ok(ObjectiveAcceptanceCriterion {
                    id: new_id(
                        ObjectiveAcceptanceCriterionId::PREFIX,
                        ObjectiveAcceptanceCriterionId::parse,
                    )?,
                    objective_id: objective.id.clone(),
                    plan_id: plan_id.clone(),
                    description: criterion.clone(),
                    status: ObjectiveAcceptanceStatus::Pending,
                    last_evaluated_at: None,
                })
            })
            .collect::<HarnessResult<Vec<_>>>()?;
        let validation_commands = response
            .objective
            .validation_commands
            .iter()
            .map(|command| {
                let review = policy.classify(command);
                Ok(ObjectiveValidationCommand {
                    id: new_id(
                        ObjectiveValidationCommandId::PREFIX,
                        ObjectiveValidationCommandId::parse,
                    )?,
                    objective_id: objective.id.clone(),
                    plan_id: plan_id.clone(),
                    command: command.clone(),
                    source: ObjectiveValidationCommandSource::Planner,
                    review_status: review.review_status(),
                    review_reason: (!review.reasons.is_empty()).then(|| review.reasons.join("; ")),
                    created_at: created_at.clone(),
                })
            })
            .collect::<HarnessResult<Vec<_>>>()?;

        let mut key_to_task_id = BTreeMap::new();
        for task in &response.tasks {
            key_to_task_id.insert(
                task.task_key.clone(),
                new_id(TaskId::PREFIX, TaskId::parse)?,
            );
        }

        let mut generated_tasks = Vec::new();
        for (sequence, task) in response.tasks.iter().enumerate() {
            let task_id = key_to_task_id
                .get(&task.task_key)
                .expect("task key was prepopulated")
                .clone();
            let mut trusted_validation_commands = Vec::new();
            let mut reviewed_validation_commands = Vec::new();
            for command in &task.validation {
                let review = policy.classify(command);
                if review.executable_argv().is_some() {
                    trusted_validation_commands.push(command.clone());
                } else {
                    reviewed_validation_commands.push(ObjectiveTaskValidationCommand {
                        id: new_id(
                            ObjectiveValidationCommandId::PREFIX,
                            ObjectiveValidationCommandId::parse,
                        )?,
                        objective_id: objective.id.clone(),
                        task_id: task_id.clone(),
                        command: command.clone(),
                        review_status: review.review_status(),
                        review_reason: Some(review.reasons.join("; ")),
                        created_at: created_at.clone(),
                    });
                }
            }
            if trusted_validation_commands.is_empty() {
                return Err(HarnessError::SecurityPolicy(format!(
                    "planner task {} has no trusted validation command",
                    task.task_key
                )));
            }
            generated_tasks.push(NewGeneratedTask {
                task: Task {
                    id: task_id.clone(),
                    title: self.redact_text(&task.title),
                    goal: self.redact_text(&task.goal),
                    status: TaskStatus::Ready,
                    repo_root: repo_root.to_string(),
                    worktree_path: None,
                    branch: None,
                    base_ref: Some("HEAD".to_string()),
                    base_commit: None,
                    last_seen_head: None,
                    max_attempts: worker_attempt_budget,
                    lease_owner: None,
                    lease_acquired_at: None,
                    lease_expires_at: None,
                    heartbeat_at: None,
                    lock_version: 0,
                    created_at: created_at.clone(),
                    updated_at: created_at.clone(),
                },
                task_key: task.task_key.clone(),
                parallel_group: Some(task.parallel_group.clone()),
                owned_paths_json: serde_json::to_string(&task.owned_paths).map_err(|error| {
                    HarnessError::External(format!("serialize owned paths: {error}"))
                })?,
                sequence: sequence as u32,
                worker_attempt_budget,
                trusted_validation_commands,
                reviewed_validation_commands,
            });
        }

        let dependencies = response
            .tasks
            .iter()
            .flat_map(|task| {
                task.depends_on
                    .iter()
                    .map(|dependency| NewObjectiveTaskDependency {
                        task_id: key_to_task_id
                            .get(&task.task_key)
                            .expect("task key exists")
                            .clone(),
                        depends_on_task_id: key_to_task_id
                            .get(dependency)
                            .expect("dependency key validated")
                            .clone(),
                    })
            })
            .collect::<Vec<_>>();

        Ok(ObjectivePlanBundle {
            objective_updated_at: created_at.clone(),
            plan,
            acceptance_criteria,
            validation_commands,
            generated_tasks,
            dependencies,
            artifacts: vec![request_artifact.clone(), response_artifact.clone()],
            exchange: PlannerExchange {
                id: exchange_id,
                objective_id: objective.id.clone(),
                kind: PlannerExchangeKind::InitialPlan,
                ticket_id: None,
                model: planner_model,
                system_prompt_version: crate::prompts::PROMPT_CONTRACT_VERSION.to_string(),
                request_objective_artifact_id: Some(request_artifact.id),
                response_objective_artifact_id: Some(response_artifact.id.clone()),
                status: "accepted".to_string(),
                error: None,
                created_at: created_at.clone(),
            },
            messages: vec![
                NewObjectiveMessage {
                    id: new_id(ObjectiveMessageId::PREFIX, ObjectiveMessageId::parse)?,
                    objective_id: objective.id.clone(),
                    role: "user".to_string(),
                    kind: "prompt".to_string(),
                    content_objective_artifact_id: None,
                    content_preview: objective.prompt.chars().take(512).collect(),
                    created_at: created_at.clone(),
                },
                NewObjectiveMessage {
                    id: new_id(ObjectiveMessageId::PREFIX, ObjectiveMessageId::parse)?,
                    objective_id: objective.id.clone(),
                    role: "planner".to_string(),
                    kind: "response".to_string(),
                    content_objective_artifact_id: Some(response_artifact.id.clone()),
                    content_preview: response.objective.summary.chars().take(512).collect(),
                    created_at: created_at.clone(),
                },
            ],
            events: vec![ObjectiveEvent {
                id: new_id(ObjectiveEventId::PREFIX, ObjectiveEventId::parse)?,
                objective_id: objective.id.clone(),
                event_type: "objective.plan_accepted".to_string(),
                message: "objective plan accepted".to_string(),
                payload_json: json!({
                    "task_count": response.tasks.len(),
                    "acceptance_criteria_count": response.objective.acceptance_criteria.len(),
                })
                .to_string(),
                created_at,
            }],
        })
    }

    pub fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
        self.store.list_tickets(None, None)
    }

    pub fn get_ticket(&self, ticket_id: &TicketId) -> HarnessResult<Ticket> {
        self.store.get_ticket(ticket_id)
    }

    pub fn run_task(
        &self,
        task_id: &TaskId,
        options: TaskRunOptions,
    ) -> HarnessResult<CommandResult> {
        self.run_or_resume(task_id, None, options.max_attempts, options.model)
    }

    pub fn resume_task(
        &self,
        task_id: &TaskId,
        options: ResumeTaskOptions,
    ) -> HarnessResult<CommandResult> {
        self.run_or_resume(
            task_id,
            Some(options.ticket_id),
            options.max_attempts,
            options.model,
        )
    }

    pub fn resolve_ticket(
        &self,
        ticket_id: &TicketId,
        options: TicketResolveOptions,
    ) -> HarnessResult<CommandResult> {
        let ticket = self.store.get_ticket(ticket_id)?;
        let owner = owner(&ticket.task_id);
        self.store.acquire_task_lease(&ticket.task_id, &owner)?;
        let result = self.resolve_ticket_with_lease(ticket_id, options.model, &owner);
        let release = self.store.release_task_lease(&ticket.task_id, &owner);
        match (result, release) {
            (Ok(result), Ok(())) => Ok(result),
            (Ok(_), Err(err)) | (Err(err), _) => Err(err),
        }
    }

    fn run_or_resume(
        &self,
        task_id: &TaskId,
        resume_ticket: Option<Option<TicketId>>,
        max_attempts: Option<u32>,
        model: Option<String>,
    ) -> HarnessResult<CommandResult> {
        let owner = owner(task_id);
        self.store.acquire_task_lease(task_id, &owner)?;
        let result = self.run_with_lease(task_id, resume_ticket, max_attempts, model, &owner);
        let release = self.store.release_task_lease(task_id, &owner);
        match (result, release) {
            (Ok(result), Ok(())) => Ok(result),
            (Ok(_), Err(err)) | (Err(err), _) => Err(err),
        }
    }

    fn run_with_lease(
        &self,
        task_id: &TaskId,
        resume_ticket: Option<Option<TicketId>>,
        max_attempts: Option<u32>,
        model: Option<String>,
        owner: &str,
    ) -> HarnessResult<CommandResult> {
        let mut task = self.store.get_task(task_id)?;
        let mut parent = None;
        let mut escalation_cycle = 0;
        let resume = if let Some(ticket_id) = resume_ticket {
            if task.status != TaskStatus::Stuck {
                return Err(HarnessError::Conflict(format!(
                    "task {} is not stuck",
                    task.id
                )));
            }
            let selected = self.select_resolution(&task.id, ticket_id.as_ref())?;
            parent = self.store.latest_run_for_task(&task.id)?;
            escalation_cycle = parent
                .as_ref()
                .map_or(0, |run| run.escalation_cycle.saturating_add(1));
            if escalation_cycle > self.config.orchestrator.max_escalation_cycles {
                return Err(HarnessError::Conflict(
                    "max escalation cycles exceeded".to_string(),
                ));
            }
            self.store
                .transition_task(&task.id, TaskStatus::Stuck, TaskStatus::Running, owner)?;
            Some(selected)
        } else {
            if task.status != TaskStatus::Ready {
                return Err(HarnessError::Conflict(format!(
                    "task {} is not ready",
                    task.id
                )));
            }
            self.store
                .transition_task(&task.id, TaskStatus::Ready, TaskStatus::Running, owner)?;
            None
        };
        task = self.store.get_task(&task.id)?;
        let validations = self
            .store
            .list_validation_commands(&task.id)?
            .into_iter()
            .map(|cmd| cmd.command)
            .collect::<Vec<_>>();
        if validations.is_empty() {
            return Err(HarnessError::Usage(
                "task has no validation commands".to_string(),
            ));
        }

        let recorded = task.worktree_path.as_ref().map(|path| RecordedWorktree {
            path: path.clone(),
            branch: task
                .branch
                .clone()
                .unwrap_or_else(|| format!("harness/{}", task.id)),
            base_ref: task.base_ref.clone(),
            base_commit: task.base_commit.clone(),
            last_seen_head: task.last_seen_head.clone(),
        });
        let worktree = self.workspace.ensure_task_worktree(WorktreeRequest {
            repo_root: task.repo_root.clone(),
            worktree_root: self.config.workspace.worktree_root.clone(),
            task_id: task.id.clone(),
            base_ref: task.base_ref.clone(),
            recorded,
        })?;
        task.worktree_path = Some(worktree.path.clone());
        task.branch = Some(worktree.branch.clone());
        task.base_ref = Some(worktree.base_ref.clone());
        task.base_commit = Some(worktree.base_commit.clone());
        task.last_seen_head = Some(worktree.head);
        task.updated_at = now();
        self.store.update_task(task.clone(), owner)?;

        let run = Run {
            id: new_id(RunId::PREFIX, RunId::parse)?,
            task_id: task.id.clone(),
            parent_run_id: parent.map(|run| run.id),
            status: RunStatus::Running,
            repo_root: task.repo_root.clone(),
            base_ref: task.base_ref.clone(),
            base_commit: worktree.base_commit,
            dirty_state_summary: None,
            current_phase: Some("attempt".to_string()),
            escalation_cycle,
            started_at: now(),
            finished_at: None,
            final_diff_path: None,
            last_error: None,
        };
        self.store.insert_run(run.clone(), owner)?;
        self.attempt_loop(task, run, validations, max_attempts, model, resume, owner)
    }

    fn attempt_loop(
        &self,
        task: Task,
        run: Run,
        validations: Vec<String>,
        max_attempts: Option<u32>,
        model: Option<String>,
        resume: Option<ResumeResolution>,
        owner: &str,
    ) -> HarnessResult<CommandResult> {
        let max_attempts = max_attempts.unwrap_or(task.max_attempts).max(1);
        let model = model.unwrap_or_else(|| self.config.providers.ollama.default_model.clone());
        let worktree = task.worktree_path.clone().unwrap_or_default();
        let mut ctx = AttemptContext {
            attempt_count: 0,
            validation_log: None,
            validation_log_path: None,
            validation_log_hash: None,
            validation_exit_code: None,
            last_validation_command: validations.last().cloned(),
            last_validation_cwd: Some(worktree.clone()),
            summaries: Vec::new(),
            last_response: None,
            last_response_path: None,
            last_response_hash: None,
        };
        let redacted_title = self.redact_text(&task.title);
        let redacted_goal = self.redact_text(&task.goal);
        let redacted_validations = validations
            .iter()
            .map(|command| self.redact_text(command))
            .collect::<Vec<_>>();
        let redacted_resume = resume.as_ref().map(|resolution| ResumeResolution {
            resolution: resolution.resolution.clone(),
            text: self.redact_text(&resolution.text),
        });
        let mut resume_consumed = false;
        let mut provider_failures = 0_u32;
        let mut patch_rejections = 0_u32;
        for attempt_number in 1..=max_attempts {
            self.store.heartbeat_task_lease(&task.id, owner)?;
            let attempt_id = new_id(AttemptId::PREFIX, AttemptId::parse)?;
            let prompt = build_ollama_worker_prompt(OllamaPromptRequest {
                title: redacted_title.clone(),
                goal: redacted_goal.clone(),
                validation_commands: redacted_validations.clone(),
                current_diff: self
                    .workspace
                    .capture_diff(&worktree, &run.id)
                    .ok()
                    .map(|diff| self.redact_text(&diff)),
                validation_log: ctx.validation_log.as_ref().map(|log| self.redact_text(log)),
                prior_attempt_summaries: ctx
                    .summaries
                    .iter()
                    .map(|summary| self.redact_text(summary))
                    .collect(),
                ticket_resolutions: redacted_resume
                    .iter()
                    .map(|resolution| resolution.text.clone())
                    .collect(),
            })?;
            let prompt_path = self.write_artifact(
                &task.id,
                Some(&run.id),
                Some(&attempt_id),
                None,
                "prompt.md",
                &prompt.input,
            )?;
            let response = match block_on(self.ollama.complete(ModelRequest {
                model: model.clone(),
                system: prompt.system,
                input: prompt.input,
                temperature: Some(self.config.providers.ollama.temperature),
                max_output_tokens: Some(self.config.providers.ollama.num_predict),
                metadata: BTreeMap::new(),
            })) {
                Ok(response) => {
                    if let Some(resume) = &resume
                        && !resume_consumed
                    {
                        self.store.mark_ticket_resolution_consumed(
                            &resume.resolution.id,
                            &now(),
                            owner,
                        )?;
                        resume_consumed = true;
                    }
                    response
                }
                Err(error) => {
                    let retryable = error.is_retryable();
                    provider_failures += 1;
                    let reason = provider_error(error);
                    self.insert_attempt(
                        &run.id,
                        attempt_id.clone(),
                        attempt_number,
                        "ollama",
                        &model,
                        AttemptStatus::Failed,
                        Some(prompt_path.clone()),
                        None,
                        None,
                        None,
                        None,
                        Some(reason.clone()),
                        None,
                        owner,
                    )?;
                    let prompt_artifact = self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "prompt",
                        prompt_path,
                        owner,
                    )?;
                    self.write_attempt_manifest(
                        &task.id,
                        &run.id,
                        &attempt_id,
                        &[prompt_artifact],
                        Some("ollama"),
                        Some(&model),
                        None,
                    )
                    .and_then(|path| {
                        self.insert_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(&attempt_id),
                            None,
                            "manifest",
                            path,
                            owner,
                        )
                    })?;
                    ctx.summaries
                        .push(format!("attempt {attempt_number}: {reason}"));
                    if !retryable
                        || provider_failures >= self.config.orchestrator.max_provider_failures
                        || attempt_number == max_attempts
                    {
                        return self.stuck_result(
                            &task,
                            &run,
                            "provider_failure",
                            reason,
                            "How should the local provider failure be resolved?".to_string(),
                            &ctx,
                            owner,
                        );
                    }
                    continue;
                }
            };
            let response_path = self.write_artifact(
                &task.id,
                Some(&run.id),
                Some(&attempt_id),
                None,
                "response.txt",
                &response.text,
            )?;
            ctx.attempt_count = attempt_number;
            ctx.last_response = Some(response.text.clone());
            ctx.last_response_path = Some(response_path.clone());
            ctx.last_response_hash = Some(file_hash(&response_path)?);
            match self.handle_response(
                &task,
                &run,
                &attempt_id,
                &response,
                &worktree,
                &validations,
            )? {
                Ok(outcome) if outcome.status == AttemptStatus::Complete => {
                    let final_diff = self.workspace.capture_diff(&worktree, &run.id)?;
                    let final_path = self.write_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "final.diff",
                        &final_diff,
                    )?;
                    self.insert_attempt(
                        &run.id,
                        attempt_id.clone(),
                        attempt_number,
                        &response.provider,
                        &response.model,
                        outcome.status,
                        Some(prompt_path.clone()),
                        Some(response_path.clone()),
                        outcome.patch_path.clone(),
                        None,
                        Some(0),
                        None,
                        None,
                        owner,
                    )?;
                    let mut artifacts = Vec::new();
                    artifacts.push(self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "prompt",
                        prompt_path,
                        owner,
                    )?);
                    artifacts.push(self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "response",
                        response_path,
                        owner,
                    )?);
                    if let Some(path) = outcome.patch_path {
                        artifacts.push(self.insert_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(&attempt_id),
                            None,
                            "patch",
                            path,
                            owner,
                        )?);
                    }
                    if let Some(path) = outcome.apply_check_stderr_path {
                        artifacts.push(self.insert_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(&attempt_id),
                            None,
                            "git_apply_check_stderr",
                            path,
                            owner,
                        )?);
                    }
                    if let Some(path) = outcome.apply_stderr_path {
                        artifacts.push(self.insert_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(&attempt_id),
                            None,
                            "git_apply_stderr",
                            path,
                            owner,
                        )?);
                    }
                    artifacts.push(self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "final_diff",
                        final_path.clone(),
                        owner,
                    )?);
                    let manifest_path = self.write_attempt_manifest(
                        &task.id,
                        &run.id,
                        &attempt_id,
                        &artifacts,
                        Some(&response.provider),
                        Some(&response.model),
                        response.response_id.clone(),
                    )?;
                    self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "manifest",
                        manifest_path,
                        owner,
                    )?;
                    self.store.update_run(
                        &run.id,
                        Some(RunStatus::Running),
                        RunUpdate {
                            status: Some(RunStatus::Complete),
                            current_phase: Some("complete".to_string()),
                            finished_at: Some(now()),
                            final_diff_path: Some(final_path),
                            ..RunUpdate::default()
                        },
                        owner,
                    )?;
                    let task = self.store.transition_task(
                        &task.id,
                        TaskStatus::Running,
                        TaskStatus::Complete,
                        owner,
                    )?;
                    return Ok(CommandResult::with_data(
                        CommandExit::new(
                            CommandStatus::Complete,
                            0,
                            Some(format!("task {} complete", task.id)),
                        ),
                        json!({"task_id": task.id.as_str(), "run_id": run.id.as_str()}),
                    )
                    .with_event(CommandEvent::info("task.complete", "task complete")));
                }
                Ok(outcome) => {
                    if let Some(log) = &outcome.validation_log {
                        ctx.validation_log = Some(log.clone());
                    }
                    let validation_log_path = outcome
                        .validation_log
                        .as_ref()
                        .map(|log| {
                            self.write_artifact(
                                &task.id,
                                Some(&run.id),
                                Some(&attempt_id),
                                None,
                                "validation.log",
                                log,
                            )
                        })
                        .transpose()?;
                    ctx.validation_exit_code = outcome.validation_exit_code;
                    if let Some(command) = &outcome.validation_command {
                        ctx.last_validation_command = Some(command.clone());
                    }
                    if let Some(cwd) = &outcome.validation_cwd {
                        ctx.last_validation_cwd = Some(cwd.clone());
                    }
                    ctx.validation_log_path = validation_log_path.clone();
                    ctx.validation_log_hash = validation_log_path
                        .as_ref()
                        .map(|path| file_hash(path))
                        .transpose()?;
                    let reason = outcome
                        .reason
                        .clone()
                        .or_else(|| outcome.validation_log.clone())
                        .unwrap_or_else(|| outcome.status.to_string());
                    self.insert_attempt(
                        &run.id,
                        attempt_id.clone(),
                        attempt_number,
                        &response.provider,
                        &response.model,
                        outcome.status,
                        Some(prompt_path.clone()),
                        Some(response_path.clone()),
                        outcome.patch_path.clone(),
                        validation_log_path.clone(),
                        outcome.validation_exit_code,
                        Some(reason.clone()),
                        outcome.apply_error,
                        owner,
                    )?;
                    let mut artifacts = Vec::new();
                    artifacts.push(self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "prompt",
                        prompt_path,
                        owner,
                    )?);
                    artifacts.push(self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "response",
                        response_path,
                        owner,
                    )?);
                    if let Some(path) = outcome.patch_path {
                        artifacts.push(self.insert_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(&attempt_id),
                            None,
                            "patch",
                            path,
                            owner,
                        )?);
                    }
                    if let Some(path) = outcome.apply_check_stderr_path {
                        artifacts.push(self.insert_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(&attempt_id),
                            None,
                            "git_apply_check_stderr",
                            path,
                            owner,
                        )?);
                    }
                    if let Some(path) = outcome.apply_stderr_path {
                        artifacts.push(self.insert_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(&attempt_id),
                            None,
                            "git_apply_stderr",
                            path,
                            owner,
                        )?);
                    }
                    if let Some(path) = validation_log_path {
                        artifacts.push(self.insert_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(&attempt_id),
                            None,
                            "validation_log",
                            path,
                            owner,
                        )?);
                    }
                    let manifest_path = self.write_attempt_manifest(
                        &task.id,
                        &run.id,
                        &attempt_id,
                        &artifacts,
                        Some(&response.provider),
                        Some(&response.model),
                        response.response_id.clone(),
                    )?;
                    self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "manifest",
                        manifest_path,
                        owner,
                    )?;
                    ctx.summaries
                        .push(format!("attempt {attempt_number}: {reason}"));
                    if outcome.status == AttemptStatus::InvalidResponse {
                        return self.stuck_result(
                            &task,
                            &run,
                            "invalid_response",
                            reason,
                            "How should this task continue?".to_string(),
                            &ctx,
                            owner,
                        );
                    }
                    if outcome.status == AttemptStatus::PatchRejected {
                        patch_rejections += 1;
                    }
                    if (outcome.status == AttemptStatus::PatchRejected && patch_rejections >= 2)
                        || attempt_number == max_attempts
                    {
                        let blocked = match outcome.status {
                            AttemptStatus::PatchRejected => "patch_rejected",
                            AttemptStatus::ValidationFailed => "validation_failed",
                            _ => "attempt_failed",
                        };
                        return self.stuck_result(
                            &task,
                            &run,
                            blocked,
                            reason,
                            "How should this task continue?".to_string(),
                            &ctx,
                            owner,
                        );
                    }
                }
                Err(stuck) => {
                    self.insert_attempt(
                        &run.id,
                        attempt_id.clone(),
                        attempt_number,
                        &response.provider,
                        &response.model,
                        AttemptStatus::Failed,
                        Some(prompt_path.clone()),
                        Some(response_path.clone()),
                        None,
                        None,
                        None,
                        Some(stuck.0.clone()),
                        None,
                        owner,
                    )?;
                    let mut artifacts = Vec::new();
                    artifacts.push(self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "prompt",
                        prompt_path,
                        owner,
                    )?);
                    artifacts.push(self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "response",
                        response_path,
                        owner,
                    )?);
                    let manifest_path = self.write_attempt_manifest(
                        &task.id,
                        &run.id,
                        &attempt_id,
                        &artifacts,
                        Some(&response.provider),
                        Some(&response.model),
                        response.response_id.clone(),
                    )?;
                    self.insert_artifact(
                        &task.id,
                        Some(&run.id),
                        Some(&attempt_id),
                        None,
                        "manifest",
                        manifest_path,
                        owner,
                    )?;
                    return self.stuck_result(
                        &task,
                        &run,
                        "model_stuck",
                        stuck.0,
                        stuck.1,
                        &ctx,
                        owner,
                    );
                }
            }
        }
        Err(HarnessError::External(
            "attempt loop did not finish".to_string(),
        ))
    }

    fn handle_response(
        &self,
        task: &Task,
        run: &Run,
        attempt_id: &AttemptId,
        response: &ModelResponse,
        worktree: &str,
        validations: &[String],
    ) -> HarnessResult<Result<ResponseOutcome, (String, String)>> {
        match parse_ollama_response(&response.text) {
            Err(error) => Ok(Ok(ResponseOutcome {
                status: AttemptStatus::InvalidResponse,
                reason: Some(error.to_string()),
                validation_log: None,
                validation_exit_code: None,
                validation_command: None,
                validation_cwd: None,
                apply_error: None,
                patch_path: None,
                apply_check_stderr_path: None,
                apply_stderr_path: None,
            })),
            Ok(OllamaResponse::Stuck(stuck)) => Ok(Err((stuck.reason, stuck.question))),
            Ok(OllamaResponse::Patch(patch)) => {
                let diff = patch.diff;
                let patch_path = self.write_artifact(
                    &task.id,
                    Some(&run.id),
                    Some(attempt_id),
                    None,
                    "patch.diff",
                    &diff,
                )?;
                let patch_check = PatchCheck {
                    worktree_path: worktree.to_string(),
                    diff,
                    max_patch_bytes: self.config.orchestrator.max_patch_bytes,
                    max_files_changed: self.config.orchestrator.max_files_changed,
                };
                let check = match self.workspace.check_patch(patch_check.clone()) {
                    Ok(check) => check,
                    Err(error) => {
                        let check_error = error.to_string();
                        let check_stderr_path = self.write_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(attempt_id),
                            None,
                            "git_apply_check.stderr",
                            &check_error,
                        )?;
                        return Ok(Ok(ResponseOutcome {
                            status: AttemptStatus::PatchRejected,
                            reason: Some("patch check rejected".to_string()),
                            validation_log: None,
                            validation_exit_code: None,
                            validation_command: None,
                            validation_cwd: None,
                            apply_error: Some(check_error),
                            patch_path: Some(patch_path),
                            apply_check_stderr_path: Some(check_stderr_path),
                            apply_stderr_path: None,
                        }));
                    }
                };
                let apply = self.workspace.apply_patch(patch_check);
                let (apply_check_stderr_path, apply_stderr_path) = match apply {
                    Ok(result) => {
                        let check_path = self.write_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(attempt_id),
                            None,
                            "git_apply_check.stderr",
                            &check.stderr,
                        )?;
                        let apply_path = self.write_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(attempt_id),
                            None,
                            "git_apply.stderr",
                            &result.stderr,
                        )?;
                        (Some(check_path), apply_path)
                    }
                    Err(error) => {
                        let apply_error = error.to_string();
                        let apply_stderr_path = self.write_artifact(
                            &task.id,
                            Some(&run.id),
                            Some(attempt_id),
                            None,
                            "git_apply.stderr",
                            &apply_error,
                        )?;
                        return Ok(Ok(ResponseOutcome {
                            status: AttemptStatus::PatchRejected,
                            reason: Some("patch apply rejected".to_string()),
                            validation_log: None,
                            validation_exit_code: None,
                            validation_command: None,
                            validation_cwd: None,
                            apply_error: Some(apply_error),
                            patch_path: Some(patch_path),
                            apply_check_stderr_path: Some(self.write_artifact(
                                &task.id,
                                Some(&run.id),
                                Some(attempt_id),
                                None,
                                "git_apply_check.stderr",
                                &check.stderr,
                            )?),
                            apply_stderr_path: Some(apply_stderr_path),
                        }));
                    }
                };
                for command in validations {
                    let output = self.runner.run_validation(CommandSpec {
                        command: command.clone(),
                        cwd: worktree.to_string(),
                        shell_path: self.config.command.shell_path.clone(),
                        env: BTreeMap::new(),
                        timeout_seconds: self.config.orchestrator.validation_timeout_seconds,
                        max_output_bytes: self.config.orchestrator.max_validation_output_bytes,
                        stdin: CommandStdin::Null,
                        kill_process_group_on_timeout: self
                            .config
                            .command
                            .kill_process_group_on_timeout,
                    });
                    match output {
                        Ok(output) if output.exit_code == Some(0) && !output.timed_out => {}
                        Ok(output) => {
                            let log = format!(
                                "$ {command}\nexit: {:?}\nstdout:\n{}\nstderr:\n{}",
                                output.exit_code, output.stdout, output.stderr
                            );
                            let reason = if output.timed_out {
                                format!("validation command timed out: {command}")
                            } else {
                                format!(
                                    "validation command failed: {command} (exit {:?})",
                                    output.exit_code
                                )
                            };
                            return Ok(Ok(ResponseOutcome {
                                status: AttemptStatus::ValidationFailed,
                                reason: Some(reason),
                                validation_log: Some(log),
                                validation_exit_code: output.exit_code,
                                validation_command: Some(command.clone()),
                                validation_cwd: Some(worktree.to_string()),
                                apply_error: None,
                                patch_path: Some(patch_path),
                                apply_check_stderr_path,
                                apply_stderr_path: Some(apply_stderr_path),
                            }));
                        }
                        Err(error) => {
                            return Ok(Ok(ResponseOutcome {
                                status: AttemptStatus::ValidationFailed,
                                reason: Some(error.to_string()),
                                validation_log: Some(error.to_string()),
                                validation_exit_code: None,
                                validation_command: Some(command.clone()),
                                validation_cwd: Some(worktree.to_string()),
                                apply_error: None,
                                patch_path: Some(patch_path),
                                apply_check_stderr_path,
                                apply_stderr_path: Some(apply_stderr_path),
                            }));
                        }
                    }
                }
                Ok(Ok(ResponseOutcome {
                    status: AttemptStatus::Complete,
                    reason: None,
                    validation_log: None,
                    validation_exit_code: Some(0),
                    validation_command: None,
                    validation_cwd: None,
                    apply_error: None,
                    patch_path: Some(patch_path),
                    apply_check_stderr_path,
                    apply_stderr_path: Some(apply_stderr_path),
                }))
            }
        }
    }

    fn redact_text(&self, text: &str) -> String {
        self.redactor.redact(text).text
    }

    fn stuck_result(
        &self,
        task: &Task,
        run: &Run,
        blocked_on: &str,
        reason: String,
        question: String,
        ctx: &AttemptContext,
        owner: &str,
    ) -> HarnessResult<CommandResult> {
        let ticket_id = new_id(TicketId::PREFIX, TicketId::parse)?;
        let current_diff_path = task
            .worktree_path
            .as_ref()
            .and_then(|path| self.workspace.capture_diff(path, &run.id).ok())
            .map(|diff| {
                self.write_artifact(
                    &task.id,
                    Some(&run.id),
                    None,
                    Some(&ticket_id),
                    "current.diff",
                    &diff,
                )
            })
            .transpose()?;
        let current_diff_hash = current_diff_path
            .as_ref()
            .map(|path| file_hash(path))
            .transpose()?;
        let evidence = self
            .redactor
            .redact(&stable_ticket_evidence(
                task,
                run,
                &ticket_id,
                blocked_on,
                &reason,
                &question,
                ctx,
                current_diff_path.as_deref(),
                current_diff_hash.as_deref(),
            ))
            .text;
        let exit_code = ctx
            .validation_exit_code
            .map(|code| code.to_string())
            .unwrap_or_default();
        let failure_fingerprint = fingerprint(&[
            &normalize_fingerprint_text(&reason),
            ctx.last_validation_command.as_deref().unwrap_or_default(),
            &exit_code,
            current_diff_hash.as_deref().unwrap_or_default(),
            ctx.validation_log_hash.as_deref().unwrap_or_default(),
            ctx.last_response_hash.as_deref().unwrap_or_default(),
        ]);
        let ticket = self.store.create_or_get_ticket(
            Ticket {
                id: ticket_id.clone(),
                task_id: task.id.clone(),
                run_id: run.id.clone(),
                status: TicketStatus::Open,
                blocked_on: blocked_on.to_string(),
                question,
                reason: reason.clone(),
                evidence_json: evidence,
                failure_fingerprint,
                created_at: now(),
                resolved_at: None,
            },
            owner,
        )?;
        if let Some(path) = current_diff_path {
            self.insert_artifact(
                &task.id,
                Some(&run.id),
                None,
                Some(&ticket_id),
                "current_diff",
                path,
                owner,
            )?;
        }
        self.store.update_run(
            &run.id,
            Some(RunStatus::Running),
            RunUpdate {
                status: Some(RunStatus::Stuck),
                current_phase: Some("stuck".to_string()),
                finished_at: Some(now()),
                last_error: Some(reason),
                ..RunUpdate::default()
            },
            owner,
        )?;
        self.store
            .transition_task(&task.id, TaskStatus::Running, TaskStatus::Stuck, owner)?;
        self.store.insert_event(
            Event {
                id: new_id(EventId::PREFIX, EventId::parse)?,
                task_id: Some(task.id.clone()),
                run_id: Some(run.id.clone()),
                kind: "ticket.created".to_string(),
                level: EventLevel::Warn,
                message: format!("created ticket {}", ticket.id),
                artifact_path: None,
                created_at: now(),
            },
            owner,
        )?;
        Ok(CommandResult::with_data(CommandExit::new(CommandStatus::Stuck, 10, Some(format!("task {} is stuck; ticket {}", task.id, ticket.id))), json!({"task_id": task.id.as_str(), "run_id": run.id.as_str(), "ticket_id": ticket.id.as_str(), "blocked_on": ticket.blocked_on})).with_event(CommandEvent::warn("task.stuck", "task stuck")))
    }

    fn resolve_ticket_with_lease(
        &self,
        ticket_id: &TicketId,
        model: Option<String>,
        owner: &str,
    ) -> HarnessResult<CommandResult> {
        let mut ticket = self.store.get_ticket(ticket_id)?;
        ticket = match ticket.status {
            TicketStatus::Open => self.store.transition_ticket(
                ticket_id,
                TicketStatus::Open,
                TicketStatus::Resolving,
                owner,
            )?,
            TicketStatus::Failed => self.store.transition_ticket(
                ticket_id,
                TicketStatus::Failed,
                TicketStatus::Resolving,
                owner,
            )?,
            TicketStatus::Resolving => ticket,
            _ => {
                return Err(HarnessError::Conflict(format!(
                    "ticket {} cannot be resolved",
                    ticket.id
                )));
            }
        };
        let task = self.store.get_task(&ticket.task_id)?;
        let run = self.store.get_run(&ticket.run_id)?;
        let mut prompt_ticket = ticket.clone();
        prompt_ticket.blocked_on = self.redact_text(&prompt_ticket.blocked_on);
        prompt_ticket.question = self.redact_text(&prompt_ticket.question);
        prompt_ticket.reason = self.redact_text(&prompt_ticket.reason);
        prompt_ticket.evidence_json = self.redact_text(&prompt_ticket.evidence_json);
        let mut prompt_task = task.clone();
        prompt_task.title = self.redact_text(&prompt_task.title);
        prompt_task.goal = self.redact_text(&prompt_task.goal);
        let prompt = build_ticket_prompt(TicketPromptRequest {
            ticket: prompt_ticket,
            task: prompt_task,
            run: run.clone(),
            evidence_json: self.redact_text(&ticket.evidence_json),
            failing_command: None,
            current_diff: task
                .worktree_path
                .as_ref()
                .and_then(|path| self.workspace.capture_diff(path, &run.id).ok())
                .map(|diff| self.redact_text(&diff)),
            validation_log: None,
            prior_attempt_summaries: self
                .store
                .list_attempts(&run.id)?
                .into_iter()
                .map(|attempt| format!("attempt {}: {}", attempt.attempt_number, attempt.status))
                .map(|summary| self.redact_text(&summary))
                .collect(),
            last_response: None,
        })?;
        let response = block_on(self.openai.complete(ModelRequest {
            model: model.unwrap_or_else(|| self.config.providers.openai.default_model.clone()),
            system: prompt.system,
            input: prompt.input,
            temperature: Some(0.0),
            max_output_tokens: Some(self.config.providers.openai.max_output_tokens),
            metadata: BTreeMap::new(),
        }))
        .map_err(|error| HarnessError::External(provider_error(error)))?;
        let resolution_id = new_id(TicketResolutionId::PREFIX, TicketResolutionId::parse)?;
        let path = self.write_artifact(
            &task.id,
            Some(&run.id),
            None,
            Some(&ticket.id),
            "resolution.md",
            &response.text,
        )?;
        let resolution = TicketResolution {
            id: resolution_id,
            ticket_id: ticket.id.clone(),
            provider: response.provider,
            model: response.model,
            response_id: response.response_id,
            resolution_path: path.clone(),
            consumed_at: None,
            created_at: now(),
        };
        self.store
            .insert_ticket_resolution(resolution.clone(), owner)?;
        let resolution_artifact = self.insert_artifact(
            &task.id,
            Some(&run.id),
            None,
            Some(&ticket.id),
            "ticket_resolution",
            path,
            owner,
        )?;
        let manifest_path = self.write_ticket_manifest(
            &task.id,
            &run.id,
            &ticket.id,
            std::slice::from_ref(&resolution_artifact),
            Some(&resolution.provider),
            Some(&resolution.model),
            resolution.response_id.clone(),
        )?;
        self.insert_artifact(
            &task.id,
            Some(&run.id),
            None,
            Some(&ticket.id),
            "manifest",
            manifest_path,
            owner,
        )?;
        Ok(CommandResult::with_data(
            CommandExit::new(
                CommandStatus::Complete,
                0,
                Some(format!("resolved ticket {}", ticket.id)),
            ),
            json!({"ticket_id": ticket.id.as_str(), "resolution_id": resolution.id.as_str()}),
        ))
    }

    fn select_resolution(
        &self,
        task_id: &TaskId,
        ticket_id: Option<&TicketId>,
    ) -> HarnessResult<ResumeResolution> {
        let resolution = if let Some(ticket_id) = ticket_id {
            let ticket = self.store.get_ticket(ticket_id)?;
            if &ticket.task_id != task_id {
                return Err(HarnessError::Conflict(
                    "ticket does not belong to task".to_string(),
                ));
            }
            TaskStore::latest_unconsumed_resolution_for_ticket(self.store.as_ref(), ticket_id)?
        } else {
            let run =
                self.store
                    .latest_run_for_task(task_id)?
                    .ok_or_else(|| HarnessError::NotFound {
                        kind: "run",
                        id: task_id.as_str().to_string(),
                    })?;
            self.store.latest_unconsumed_resolution_for_run(&run.id)?
        }
        .ok_or_else(|| HarnessError::NotFound {
            kind: "ticket_resolution",
            id: task_id.as_str().to_string(),
        })?;
        let text = fs::read_to_string(&resolution.resolution_path)
            .map_err(|error| HarnessError::External(format!("read resolution: {error}")))?;
        Ok(ResumeResolution { resolution, text })
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_attempt(
        &self,
        run_id: &RunId,
        id: AttemptId,
        number: u32,
        provider: &str,
        model: &str,
        status: AttemptStatus,
        prompt_path: Option<String>,
        response_path: Option<String>,
        patch_path: Option<String>,
        validation_log_path: Option<String>,
        validation_exit_code: Option<i32>,
        failure_reason: Option<String>,
        apply_error: Option<String>,
        owner: &str,
    ) -> HarnessResult<()> {
        let now = now();
        self.store.insert_attempt(
            Attempt {
                id,
                run_id: run_id.clone(),
                attempt_number: number,
                provider: provider.to_string(),
                model: model.to_string(),
                status,
                prompt_path,
                response_path,
                patch_path,
                validation_log_path,
                validation_exit_code,
                failure_reason,
                apply_error,
                started_at: now.clone(),
                finished_at: Some(now),
            },
            owner,
        )
    }

    fn write_artifact(
        &self,
        task_id: &TaskId,
        run_id: Option<&RunId>,
        attempt_id: Option<&AttemptId>,
        ticket_id: Option<&TicketId>,
        name: &str,
        text: &str,
    ) -> HarnessResult<String> {
        let mut dir = PathBuf::from(&self.config.workspace.state_dir)
            .join("artifacts")
            .join(task_id.as_str());
        if let Some(run_id) = run_id {
            dir = dir.join(run_id.as_str());
        }
        if let Some(attempt_id) = attempt_id {
            dir = dir.join(attempt_id.as_str());
        }
        if let Some(ticket_id) = ticket_id {
            dir = dir.join(ticket_id.as_str());
        }
        fs::create_dir_all(&dir)
            .map_err(|error| HarnessError::External(format!("create artifact dir: {error}")))?;
        let path = dir.join(name);
        fs::write(&path, self.redactor.redact(text).text)
            .map_err(|error| HarnessError::External(format!("write artifact: {error}")))?;
        Ok(path.to_string_lossy().into_owned())
    }

    fn insert_artifact(
        &self,
        task_id: &TaskId,
        run_id: Option<&RunId>,
        attempt_id: Option<&AttemptId>,
        ticket_id: Option<&TicketId>,
        kind: &str,
        path: String,
        owner: &str,
    ) -> HarnessResult<Artifact> {
        let bytes = fs::read(&path)
            .map_err(|error| HarnessError::External(format!("read artifact: {error}")))?;
        let artifact = Artifact {
            id: new_id(ArtifactId::PREFIX, ArtifactId::parse)?,
            task_id: task_id.clone(),
            run_id: run_id.cloned(),
            attempt_id: attempt_id.cloned(),
            ticket_id: ticket_id.cloned(),
            kind: kind.to_string(),
            path,
            sha256: fingerprint_bytes(&bytes),
            byte_len: bytes.len() as u64,
            created_at: now(),
        };
        self.store.insert_artifact(artifact.clone(), owner)?;
        Ok(artifact)
    }

    fn resolve_objective_prompt(&self, input: &ObjectivePromptInput) -> HarnessResult<String> {
        match input {
            ObjectivePromptInput::Text(text) => Ok(text.clone()),
            ObjectivePromptInput::File(path) => fs::read_to_string(path)
                .map_err(|error| HarnessError::External(format!("read prompt file: {error}"))),
            ObjectivePromptInput::Stdin => Err(HarnessError::Usage(
                "objective start --stdin must be resolved by the CLI input layer".to_string(),
            )),
        }
    }

    fn write_objective_artifact(
        &self,
        objective_id: &ObjectiveId,
        plan_id: Option<&ObjectivePlanId>,
        exchange_id: Option<&PlannerExchangeId>,
        kind: &str,
        name: &str,
        text: &str,
    ) -> HarnessResult<ObjectiveArtifact> {
        let dir = PathBuf::from(&self.config.workspace.state_dir)
            .join("objective-artifacts")
            .join(objective_id.as_str());
        fs::create_dir_all(&dir).map_err(|error| {
            HarnessError::External(format!("create objective artifact dir: {error}"))
        })?;
        let path = dir.join(name);
        let redacted = self.redactor.redact(text).text;
        fs::write(&path, &redacted).map_err(|error| {
            HarnessError::External(format!("write objective artifact: {error}"))
        })?;
        Ok(ObjectiveArtifact {
            id: new_id(ObjectiveArtifactId::PREFIX, ObjectiveArtifactId::parse)?,
            objective_id: objective_id.clone(),
            plan_id: plan_id.cloned(),
            planner_exchange_id: exchange_id.cloned(),
            kind: kind.to_string(),
            path: path.to_string_lossy().into_owned(),
            sha256: fingerprint_bytes(redacted.as_bytes()),
            byte_len: redacted.len() as u64,
            created_at: now(),
        })
    }

    fn persist_rejected_plan(
        &self,
        objective_id: &ObjectiveId,
        exchange_id: PlannerExchangeId,
        planner_model: String,
        request_artifact: ObjectiveArtifact,
        response_artifact: ObjectiveArtifact,
        error: String,
    ) -> HarnessResult<()> {
        let created_at = now();
        self.store.reject_objective_plan(
            objective_id,
            RejectedPlannerExchange {
                artifacts: vec![request_artifact.clone(), response_artifact.clone()],
                exchange: PlannerExchange {
                    id: exchange_id,
                    objective_id: objective_id.clone(),
                    kind: PlannerExchangeKind::InitialPlan,
                    ticket_id: None,
                    model: planner_model,
                    system_prompt_version: crate::prompts::PROMPT_CONTRACT_VERSION.to_string(),
                    request_objective_artifact_id: Some(request_artifact.id),
                    response_objective_artifact_id: Some(response_artifact.id.clone()),
                    status: "rejected".to_string(),
                    error: Some(error.clone()),
                    created_at: created_at.clone(),
                },
                messages: vec![NewObjectiveMessage {
                    id: new_id(ObjectiveMessageId::PREFIX, ObjectiveMessageId::parse)?,
                    objective_id: objective_id.clone(),
                    role: "planner".to_string(),
                    kind: "rejected_response".to_string(),
                    content_objective_artifact_id: Some(response_artifact.id),
                    content_preview: error.chars().take(512).collect(),
                    created_at: created_at.clone(),
                }],
            },
            ObjectiveEvent {
                id: new_id(ObjectiveEventId::PREFIX, ObjectiveEventId::parse)?,
                objective_id: objective_id.clone(),
                event_type: "objective.plan_rejected".to_string(),
                message: "objective plan rejected".to_string(),
                payload_json: json!({ "error": error }).to_string(),
                created_at,
            },
        )
    }

    fn persist_failed_planner_call(
        &self,
        objective_id: &ObjectiveId,
        exchange_id: PlannerExchangeId,
        planner_model: String,
        request_artifact: ObjectiveArtifact,
        error: String,
    ) -> HarnessResult<()> {
        let created_at = now();
        self.store
            .insert_objective_artifact(request_artifact.clone())?;
        self.store.insert_planner_exchange(PlannerExchange {
            id: exchange_id,
            objective_id: objective_id.clone(),
            kind: PlannerExchangeKind::InitialPlan,
            ticket_id: None,
            model: planner_model,
            system_prompt_version: crate::prompts::PROMPT_CONTRACT_VERSION.to_string(),
            request_objective_artifact_id: Some(request_artifact.id),
            response_objective_artifact_id: None,
            status: "failed".to_string(),
            error: Some(error.clone()),
            created_at: created_at.clone(),
        })?;
        self.store.update_objective_status(
            objective_id,
            Some(ObjectiveStatus::Planning),
            ObjectiveStatusUpdate {
                status: Some(ObjectiveStatus::Failed),
                summary: Some(error.clone()),
                updated_at: Some(created_at.clone()),
                ..ObjectiveStatusUpdate::default()
            },
        )?;
        self.store.insert_objective_event(ObjectiveEvent {
            id: new_id(ObjectiveEventId::PREFIX, ObjectiveEventId::parse)?,
            objective_id: objective_id.clone(),
            event_type: "objective.failed".to_string(),
            message: "objective planner call failed".to_string(),
            payload_json: json!({ "error": error }).to_string(),
            created_at,
        })
    }

    fn write_attempt_manifest(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        attempt_id: &AttemptId,
        artifacts: &[Artifact],
        provider: Option<&str>,
        model: Option<&str>,
        response_id: Option<String>,
    ) -> HarnessResult<String> {
        self.write_manifest_artifact(
            task_id,
            Some(run_id),
            Some(attempt_id),
            None,
            artifacts,
            provider,
            model,
            response_id,
        )
    }

    fn write_ticket_manifest(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        ticket_id: &TicketId,
        artifacts: &[Artifact],
        provider: Option<&str>,
        model: Option<&str>,
        response_id: Option<String>,
    ) -> HarnessResult<String> {
        self.write_manifest_artifact(
            task_id,
            Some(run_id),
            None,
            Some(ticket_id),
            artifacts,
            provider,
            model,
            response_id,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn write_manifest_artifact(
        &self,
        task_id: &TaskId,
        run_id: Option<&RunId>,
        attempt_id: Option<&AttemptId>,
        ticket_id: Option<&TicketId>,
        artifacts: &[Artifact],
        provider: Option<&str>,
        model: Option<&str>,
        response_id: Option<String>,
    ) -> HarnessResult<String> {
        let run = run_id.and_then(|id| self.store.get_run(id).ok());
        let task = self.store.get_task(task_id).ok();
        let validation_command = self
            .store
            .list_validation_commands(task_id)
            .ok()
            .and_then(|commands| commands.first().map(|command| command.command.clone()));
        let post_attempt_head = task
            .as_ref()
            .and_then(|task| task.worktree_path.as_deref())
            .and_then(|path| self.workspace.resolve_base_commit(path, Some("HEAD")).ok());
        let context = ArtifactManifestContext {
            openai_response_id: response_id,
            base_commit: run.as_ref().map(|run| run.base_commit.clone()),
            pre_attempt_head: run.as_ref().map(|run| run.base_commit.clone()),
            post_attempt_head,
            validation_command,
            truncation: Vec::new(),
        };
        let manifest = build_artifact_manifest_with_context(
            artifacts,
            provider,
            model,
            provider,
            json!({
                "temperature": self.config.providers.ollama.temperature,
                "max_output_tokens": self.config.providers.ollama.num_predict,
            }),
            context,
        );
        let text = serde_json::to_string_pretty(&manifest)
            .map_err(|error| HarnessError::External(format!("serialize manifest: {error}")))?;
        self.write_artifact_raw(
            task_id,
            run_id,
            attempt_id,
            ticket_id,
            "manifest.json",
            &text,
        )
    }

    fn write_artifact_raw(
        &self,
        task_id: &TaskId,
        run_id: Option<&RunId>,
        attempt_id: Option<&AttemptId>,
        ticket_id: Option<&TicketId>,
        name: &str,
        text: &str,
    ) -> HarnessResult<String> {
        let mut dir = PathBuf::from(&self.config.workspace.state_dir)
            .join("artifacts")
            .join(task_id.as_str());
        if let Some(run_id) = run_id {
            dir = dir.join(run_id.as_str());
        }
        if let Some(attempt_id) = attempt_id {
            dir = dir.join(attempt_id.as_str());
        }
        if let Some(ticket_id) = ticket_id {
            dir = dir.join(ticket_id.as_str());
        }
        fs::create_dir_all(&dir)
            .map_err(|error| HarnessError::External(format!("create artifact dir: {error}")))?;
        let path = dir.join(name);
        fs::write(&path, text)
            .map_err(|error| HarnessError::External(format!("write artifact: {error}")))?;
        Ok(path.to_string_lossy().into_owned())
    }
}

fn provider_error(error: ProviderError) -> String {
    format!("provider {}: {}", error.kind.as_str(), error.message)
}

fn owner(task_id: &TaskId) -> String {
    format!("orchestrator-{}-{}", std::process::id(), task_id.as_str())
}

fn title_from_prompt(prompt: &str) -> String {
    let title = prompt
        .lines()
        .next()
        .unwrap_or("Objective")
        .trim()
        .chars()
        .take(96)
        .collect::<String>();
    if title.is_empty() {
        "Objective".to_string()
    } else {
        title
    }
}

fn objective_progress(
    kind: ObjectiveProgressKind,
    objective_id: &ObjectiveId,
    phase: ObjectiveProgressPhase,
    status: ObjectiveStatus,
    message: impl Into<String>,
) -> crate::runtime::ObjectiveProgressEvent {
    let mut event = crate::runtime::ObjectiveProgressEvent::new(
        kind,
        objective_id.clone(),
        phase,
        status,
        message,
        now(),
    );
    event.next_command = Some(format!(
        "harness objective get {} --output json",
        objective_id
    ));
    event
}

fn stream_objective_start_result(
    sink: &mut Option<&mut dyn OutputSink>,
    result: &CommandResult,
) -> HarnessResult<()> {
    let Some(sink) = sink.as_deref_mut() else {
        return Ok(());
    };
    for event in &result.events {
        if event
            .objective_progress
            .as_ref()
            .is_some_and(|progress| progress.kind == ObjectiveProgressKind::PlanningStarted)
        {
            continue;
        }
        sink.event(event)?;
    }
    Ok(())
}

fn objective_failure_result(
    objective_id: &ObjectiveId,
    planning_started: crate::runtime::ObjectiveProgressEvent,
    kind: ObjectiveProgressKind,
    phase: ObjectiveProgressPhase,
    status: ObjectiveStatus,
    error: HarnessError,
) -> CommandResult {
    let exit = exit_for_error(&error);
    let message = error.to_string();
    CommandResult::with_data(
        exit,
        json!({
            "objective_id": objective_id.as_str(),
            "status": status.as_str(),
            "terminal": true,
            "exit_reason": message,
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
    .with_event(CommandEvent::objective_progress(
        planning_started,
        CommandEventLevel::Info,
    ))
    .with_event(CommandEvent::objective_progress(
        objective_progress(kind, objective_id, phase, status, message),
        CommandEventLevel::Error,
    ))
}

fn exit_for_error(error: &HarnessError) -> CommandExit {
    match error {
        HarnessError::Usage(_)
        | HarnessError::InvalidId { .. }
        | HarnessError::InvalidStatus { .. }
        | HarnessError::InvalidConfig(_) => CommandExit::usage(error.to_string()),
        HarnessError::SecurityPolicy(_) => CommandExit::security_blocked(error.to_string()),
        HarnessError::Conflict(_) => CommandExit::leased(error.to_string()),
        HarnessError::NotFound { .. } | HarnessError::External(_) => {
            CommandExit::failure(error.to_string())
        }
    }
}

fn stable_ticket_evidence(
    task: &Task,
    run: &Run,
    ticket_id: &TicketId,
    blocked_on: &str,
    reason: &str,
    question: &str,
    ctx: &AttemptContext,
    current_diff_path: Option<&str>,
    current_diff_hash: Option<&str>,
) -> String {
    json!({
        "schema_version": "ticket-evidence-v1",
        "task_id": task.id.as_str(),
        "run_id": run.id.as_str(),
        "ticket_id": ticket_id.as_str(),
        "base_commit": run.base_commit,
        "worktree_path": task.worktree_path,
        "attempt_count": ctx.attempt_count,
        "blocked_on": blocked_on,
        "reason": reason,
        "question": question,
        "unblock_question": question,
        "current_diff_path": current_diff_path,
        "current_diff_hash": current_diff_hash,
        "last_validation_command": ctx.last_validation_command,
        "last_validation_cwd": ctx.last_validation_cwd,
        "last_validation_exit_code": ctx.validation_exit_code,
        "last_validation_log_path": ctx.validation_log_path,
        "last_validation_log_hash": ctx.validation_log_hash,
        "prior_attempt_summaries": ctx.summaries,
        "last_response_path": ctx.last_response_path,
        "last_response_hash": ctx.last_response_hash,
    })
    .to_string()
}

fn now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

pub(crate) fn orchestrator_now() -> String {
    now()
}

fn new_id<T>(prefix: &'static str, parse: impl Fn(String) -> HarnessResult<T>) -> HarnessResult<T> {
    parse(format!("{prefix}{}", encode_id(next_id())))
}

fn next_id() -> u128 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    (millis << 80) | u128::from(COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn encode_id(mut value: u128) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut out = [b'0'; 26];
    for idx in (0..26).rev() {
        out[idx] = ALPHABET[(value & 31) as usize];
        value >>= 5;
    }
    String::from_utf8(out.to_vec()).unwrap()
}

fn fingerprint(parts: &[&str]) -> String {
    fingerprint_bytes(parts.join("\n").as_bytes())
}

fn normalize_fingerprint_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn file_hash(path: impl AsRef<std::path::Path>) -> HarnessResult<String> {
    let bytes = fs::read(path.as_ref()).map_err(|error| {
        HarnessError::External(format!("read {}: {error}", path.as_ref().display()))
    })?;
    Ok(fingerprint_bytes(&bytes))
}

fn fingerprint_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn noop_waker() -> Waker {
    unsafe fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    let raw = RawWaker::new(std::ptr::null(), &VTABLE);
    unsafe { Waker::from_raw(raw) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ObjectiveValidationReviewStatus;
    use crate::providers::{FakeModelProvider, ProviderErrorKind};
    use crate::runtime::{CooperativeCancellationToken, ObjectiveSuperviseOptions, RuntimeOptions};
    use crate::state::{ObjectiveStore, SqliteTaskStore};
    use crate::workspace::{CommandOutput, PatchApplyResult, PatchCheckResult, WorktreeInfo};
    use std::sync::Mutex;

    #[derive(Debug)]
    struct FakeWorkspace {
        repo_root: String,
        worktree_root: String,
        check_errors: Mutex<Vec<String>>,
        apply_errors: Mutex<Vec<String>>,
    }

    impl FakeWorkspace {
        fn new(temp: &tempfile::TempDir) -> Self {
            let repo_root = temp.path().join("repo");
            let worktree_root = temp.path().join("worktrees");
            fs::create_dir_all(&repo_root).unwrap();
            fs::create_dir_all(&worktree_root).unwrap();
            Self {
                repo_root: repo_root.to_string_lossy().into_owned(),
                worktree_root: worktree_root.to_string_lossy().into_owned(),
                check_errors: Mutex::new(Vec::new()),
                apply_errors: Mutex::new(Vec::new()),
            }
        }
    }

    impl WorkspaceManager for FakeWorkspace {
        fn discover_repo_root(&self, repo: Option<&str>) -> HarnessResult<String> {
            Ok(repo.unwrap_or(&self.repo_root).to_string())
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
                    fs::create_dir_all(&path).unwrap();
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
            if let Some(error) = self.check_errors.lock().unwrap().pop() {
                return Err(HarnessError::External(error));
            }
            Ok(PatchCheckResult {
                files_changed: vec!["src/lib.rs".to_string()],
                stderr: String::new(),
            })
        }

        fn apply_patch(&self, patch: PatchCheck) -> HarnessResult<PatchApplyResult> {
            if let Some(error) = self.apply_errors.lock().unwrap().pop() {
                return Err(HarnessError::External(error));
            }
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

    #[derive(Debug, Default)]
    struct FakeRunner {
        outputs: Mutex<Vec<CommandOutput>>,
        errors: Mutex<Vec<String>>,
        specs: Mutex<Vec<CommandSpec>>,
    }

    impl FakeRunner {
        fn fail_once(&self) {
            self.outputs.lock().unwrap().push(CommandOutput {
                stdout: "fail".to_string(),
                stderr: "assertion".to_string(),
                exit_code: Some(101),
                duration_ms: 1,
                timed_out: false,
                truncated: false,
                truncated_bytes: 0,
            });
        }

        fn error_once(&self, message: impl Into<String>) {
            self.errors.lock().unwrap().push(message.into());
        }
    }

    impl CommandRunner for FakeRunner {
        fn run_validation(&self, spec: CommandSpec) -> HarnessResult<CommandOutput> {
            self.specs.lock().unwrap().push(spec);
            if let Some(error) = self.errors.lock().unwrap().pop() {
                return Err(HarnessError::External(error));
            }
            Ok(self
                .outputs
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| CommandOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: Some(0),
                    duration_ms: 1,
                    timed_out: false,
                    truncated: false,
                    truncated_bytes: 0,
                }))
        }

        fn run_shell_escape(&self, spec: CommandSpec) -> HarnessResult<CommandOutput> {
            self.run_validation(spec)
        }
    }

    struct Fixture {
        orchestrator: RunOrchestrator,
        store: Arc<SqliteTaskStore>,
        workspace: Arc<FakeWorkspace>,
        ollama: Arc<FakeModelProvider>,
        openai: Arc<FakeModelProvider>,
        runner: Arc<FakeRunner>,
        _temp: tempfile::TempDir,
    }

    impl Fixture {
        fn new(max_invalid: u32) -> Self {
            Self::new_with_lease_ttl(max_invalid, 300)
        }

        fn new_with_lease_ttl(max_invalid: u32, lease_ttl_secs: i64) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let workspace = Arc::new(FakeWorkspace::new(&temp));
            let store = Arc::new(
                SqliteTaskStore::in_memory()
                    .unwrap()
                    .with_lease_ttl_secs(lease_ttl_secs),
            );
            let runner = Arc::new(FakeRunner::default());
            let ollama = Arc::new(FakeModelProvider::new("fake-ollama"));
            let openai = Arc::new(FakeModelProvider::new("fake-openai"));
            let mut config = HarnessConfig::default();
            config.workspace.state_dir = temp.path().join("state").to_string_lossy().into_owned();
            config.workspace.worktree_root = workspace.worktree_root.clone();
            config.orchestrator.max_attempts = 2;
            config.orchestrator.max_invalid_responses = max_invalid;
            let orchestrator = RunOrchestrator::new(
                config,
                store.clone(),
                workspace.clone(),
                runner.clone(),
                ollama.clone(),
                openai.clone(),
            );
            Self {
                orchestrator,
                store,
                workspace,
                ollama,
                openai,
                runner,
                _temp: temp,
            }
        }

        fn task(&self) -> Task {
            self.orchestrator
                .create_task(
                    "Fix".to_string(),
                    "Make validation pass".to_string(),
                    vec!["cargo test".to_string()],
                )
                .unwrap()
        }
    }

    #[test]
    fn orchestrator_successful_fake_provider_patch_flow() {
        let fixture = Fixture::new(1);
        let task = fixture.task();
        fixture.ollama.push_text(diff_response());

        let result = fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: None,
                    model: None,
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(
            fixture.store.get_task(&task.id).unwrap().status,
            TaskStatus::Complete
        );
    }

    #[test]
    fn start_objective_creates_plan_tasks_and_single_planner_call() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());

        let result = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(7),
                max_cycles: None,
            })
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(fixture.openai.requests().len(), 1);
        assert_eq!(fixture.ollama.requests().len(), 0);
        let objectives = fixture.store.list_objectives(None).unwrap();
        assert_eq!(objectives.len(), 1);
        assert_eq!(objectives[0].status, ObjectiveStatus::Ready);
        assert!(objectives[0].active_plan_id.is_some());
        let tasks = fixture.store.list_tasks(None).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|task| task.max_attempts == 7));
        assert_eq!(
            fixture
                .store
                .list_planner_exchanges(&objectives[0].id)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            fixture
                .store
                .list_objective_artifacts(&objectives[0].id)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(result.events.len(), 2);
        assert_eq!(result.data["status"], "ready");
        assert_eq!(result.data["task_ids"].as_array().unwrap().len(), 2);

        let plan_view = fixture
            .orchestrator
            .get_objective_plan(&objectives[0].id)
            .unwrap();
        assert_eq!(plan_view.data["objective_id"], objectives[0].id.as_str());
        assert_eq!(
            plan_view.data["planner_exchanges"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(plan_view.data["artifacts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn start_objective_rejects_plan_without_trusted_task_validation() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(unsafe_validation_planner_json());

        let result = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: None,
                max_cycles: None,
            })
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::SecurityBlocked);
        assert_eq!(result.events.len(), 2);
        assert_eq!(result.events[1].kind, "objective.plan_rejected");
        let objectives = fixture.store.list_objectives(None).unwrap();
        assert_eq!(objectives.len(), 1);
        assert_eq!(objectives[0].status, ObjectiveStatus::Failed);
        assert_eq!(fixture.store.list_tasks(None).unwrap().len(), 0);
        assert_eq!(
            fixture
                .store
                .list_planner_exchanges(&objectives[0].id)
                .unwrap()[0]
                .status,
            "rejected"
        );
    }

    #[test]
    fn start_objective_marks_failed_when_planner_provider_fails() {
        let fixture = Fixture::new(1);
        fixture
            .openai
            .push_error(ProviderErrorKind::Timeout, "planner timed out");

        let result = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: None,
                max_cycles: None,
            })
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(result.events.len(), 2);
        assert_eq!(result.events[1].kind, "objective.failed");
        let objectives = fixture.store.list_objectives(None).unwrap();
        assert_eq!(objectives.len(), 1);
        assert_eq!(objectives[0].status, ObjectiveStatus::Failed);
        let exchanges = fixture
            .store
            .list_planner_exchanges(&objectives[0].id)
            .unwrap();
        assert_eq!(exchanges.len(), 1);
        assert_eq!(exchanges[0].status, "failed");
        assert_eq!(fixture.store.list_tasks(None).unwrap().len(), 0);
    }

    #[test]
    fn objective_monitor_completes_generated_tasks_sequentially() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: None,
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();

        let result = fixture
            .orchestrator
            .supervise_objective(
                &objective_id,
                ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: None,
                    max_cycles: Some(8),
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(result.data["status"], ObjectiveStatus::Complete.as_str());
        assert_eq!(
            fixture.store.get_objective(&objective_id).unwrap().status,
            ObjectiveStatus::Complete
        );
        assert_eq!(
            fixture
                .store
                .list_tasks(None)
                .unwrap()
                .iter()
                .filter(|task| task.status == TaskStatus::Complete)
                .count(),
            2
        );
        assert!(fixture.ollama.requests().len() >= 2);
        assert!(
            result
                .events
                .iter()
                .any(|event| event.kind == "objective.completed")
        );
    }

    #[test]
    fn run_one_cycle_completes_one_ready_objective_task() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: None,
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();

        let cycle = fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: None,
                    max_cycles: Some(8),
                },
            )
            .unwrap();

        assert!(matches!(
            cycle,
            crate::orchestrator::objective::ObjectiveCycleResult::Progress(_)
        ));
        let tasks = fixture.store.list_tasks(None).unwrap();
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Complete)
                .count(),
            1
        );
        assert_eq!(
            tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Ready)
                .count(),
            1
        );
    }

    #[test]
    fn objective_cancellation_marks_objective_cancelled() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: None,
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();
        let token = CooperativeCancellationToken::new();
        token.cancel();

        let result = fixture
            .orchestrator
            .supervise_objective_with_cancellation(
                &objective_id,
                ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: None,
                    max_cycles: Some(8),
                },
                &token,
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(result.data["status"], ObjectiveStatus::Cancelled.as_str());
        assert_eq!(
            fixture.store.get_objective(&objective_id).unwrap().status,
            ObjectiveStatus::Cancelled
        );
        assert!(
            result
                .events
                .iter()
                .any(|event| event.kind == "objective.cancelled")
        );
    }

    #[test]
    fn objective_monitor_resolves_stuck_ticket_and_resumes_local_worker() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture
            .openai
            .push_text_with_id("resp_resolver", valid_resolver_json());
        fixture
            .ollama
            .push_text("STUCK\nreason: need advice\nquestion: What should change?");
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(2),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();

        let result = fixture
            .orchestrator
            .supervise_objective(
                &objective_id,
                ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(1),
                    max_cycles: Some(8),
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(fixture.openai.requests().len(), 2);
        assert_eq!(
            fixture
                .openai
                .requests()
                .last()
                .unwrap()
                .metadata
                .get("role")
                .map(String::as_str),
            Some("objective_ticket_resolver")
        );
        let tasks = fixture.store.list_tasks(None).unwrap();
        assert!(tasks.iter().all(|task| task.status == TaskStatus::Complete));
        let tickets = fixture.store.list_tickets(None, None).unwrap();
        assert_eq!(tickets.len(), 1);
        assert_eq!(tickets[0].status, TicketStatus::Resolved);
        assert_eq!(
            fixture
                .store
                .list_ticket_resolutions(&tickets[0].id)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            fixture
                .store
                .list_resolver_attempts_for_ticket(&objective_id, &tickets[0].id)
                .unwrap()[0]
                .status,
            "resolved"
        );
        assert!(
            result
                .events
                .iter()
                .any(|event| event.kind == "objective.ticket_resolution_completed")
        );
    }

    #[test]
    fn objective_ticket_resolver_prompt_includes_objective_and_ticket_context() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.openai.push_text(valid_resolver_json());
        fixture
            .ollama
            .push_text("STUCK\nreason: missing command\nquestion: Which command is required?");
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(1),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();

        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(1),
                    max_cycles: Some(4),
                },
            )
            .unwrap();

        let requests = fixture.openai.requests();
        let request = requests.last().unwrap();
        assert!(request.input.contains("\"ticket_details\""));
        assert!(request.input.contains("Build a Rust CLI"));
        assert!(request.input.contains("Which command is required?"));
        assert!(request.input.contains("cargo test passes"));
    }

    #[test]
    fn objective_ticket_resolver_rejects_patch_like_output_and_does_not_loop() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.openai.push_text(patch_like_resolver_json());
        fixture
            .ollama
            .push_text("STUCK\nreason: need advice\nquestion: What should change?");
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(1),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();

        let result = fixture
            .orchestrator
            .supervise_objective(
                &objective_id,
                ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(1),
                    max_cycles: Some(4),
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(fixture.openai.requests().len(), 2);
        assert_eq!(
            fixture.store.get_objective(&objective_id).unwrap().status,
            ObjectiveStatus::Blocked
        );
        let tickets = fixture.store.list_tickets(None, None).unwrap();
        assert_eq!(tickets[0].status, TicketStatus::Failed);
        assert_eq!(
            fixture
                .store
                .list_resolver_attempts_for_ticket(&objective_id, &tickets[0].id)
                .unwrap()[0]
                .status,
            "failed"
        );
        let exchanges = fixture.store.list_planner_exchanges(&objective_id).unwrap();
        assert!(exchanges.iter().any(|exchange| {
            exchange.kind == PlannerExchangeKind::TicketResolution && exchange.status == "rejected"
        }));
    }

    #[test]
    fn objective_ticket_resolver_failed_attempt_is_not_retried_after_restart() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.openai.push_text(patch_like_resolver_json());
        fixture
            .ollama
            .push_text("STUCK\nreason: need advice\nquestion: What should change?");
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(1),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();
        fixture
            .orchestrator
            .supervise_objective(
                &objective_id,
                ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(1),
                    max_cycles: Some(4),
                },
            )
            .unwrap();
        fixture.openai.push_text(valid_resolver_json());

        let second = fixture
            .orchestrator
            .supervise_objective(
                &objective_id,
                ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(1),
                    max_cycles: Some(4),
                },
            )
            .unwrap();

        assert_eq!(second.exit.status, CommandStatus::Failed);
        assert_eq!(fixture.openai.requests().len(), 2);
        let tickets = fixture.store.list_tickets(None, None).unwrap();
        assert_eq!(
            fixture
                .store
                .list_resolver_attempts_for_ticket(&objective_id, &tickets[0].id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn objective_monitor_enforces_worker_attempt_budget_before_resume() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.openai.push_text(valid_resolver_json());
        fixture
            .ollama
            .push_text("STUCK\nreason: need advice\nquestion: What should change?");
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(1),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();

        let result = fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(1),
                    max_cycles: Some(4),
                },
            )
            .unwrap();

        let crate::orchestrator::objective::ObjectiveCycleResult::Terminal(result) = result else {
            panic!("expected terminal budget exhaustion");
        };
        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(
            fixture.store.get_objective(&objective_id).unwrap().status,
            ObjectiveStatus::Blocked
        );
        assert_eq!(fixture.ollama.requests().len(), 1);
        let tickets = fixture.store.list_tickets(None, None).unwrap();
        assert!(
            fixture
                .store
                .latest_unconsumed_resolution_for_ticket(&tickets[0].id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn objective_monitor_recovers_expired_running_task_before_scheduling() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(1),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();
        let task = fixture
            .store
            .next_ready_objective_task(&objective_id)
            .unwrap()
            .unwrap();
        let owner = "test-expired-worker";
        fixture.store.acquire_task_lease(&task.id, owner).unwrap();
        fixture
            .store
            .transition_task(&task.id, TaskStatus::Ready, TaskStatus::Running, owner)
            .unwrap();
        fixture
            .store
            .force_expire_task_lease_for_test(&task.id)
            .unwrap();

        let result = fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(1),
                    max_cycles: Some(4),
                },
            )
            .unwrap();

        let crate::orchestrator::objective::ObjectiveCycleResult::Terminal(result) = result else {
            panic!("expected terminal failed objective");
        };
        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(
            fixture.store.get_task(&task.id).unwrap().status,
            TaskStatus::Failed
        );
        assert_eq!(
            fixture.store.get_objective(&objective_id).unwrap().status,
            ObjectiveStatus::Failed
        );
        assert_eq!(fixture.ollama.requests().len(), 0);
    }

    #[test]
    fn objective_validation_passing_completes_objective() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(2),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();

        let result = fixture
            .orchestrator
            .supervise_objective(
                &objective_id,
                ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(result.data["exit_reason"], "acceptance_passed");
        assert_eq!(result.data["validation"]["status"], "passed");
        assert_eq!(
            fixture
                .store
                .list_active_objective_acceptance_criteria(&objective_id)
                .unwrap()[0]
                .status,
            ObjectiveAcceptanceStatus::Passing
        );
        assert!(
            result
                .events
                .iter()
                .any(|event| event.kind == "objective.validation_passed")
        );
    }

    #[test]
    fn acceptance_repair_task_is_created_once_on_validation_failure() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(2),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        fixture.runner.fail_once();

        let cycle = fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();

        assert!(matches!(
            cycle,
            crate::orchestrator::objective::ObjectiveCycleResult::Progress(_)
        ));
        let repair_tasks = fixture
            .store
            .list_tasks(None)
            .unwrap()
            .into_iter()
            .filter(|task| task.title.starts_with("Repair objective acceptance:"))
            .collect::<Vec<_>>();
        assert_eq!(repair_tasks.len(), 1);
        let _second = fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        let repair_tasks = fixture
            .store
            .list_tasks(None)
            .unwrap()
            .into_iter()
            .filter(|task| task.title.starts_with("Repair objective acceptance:"))
            .collect::<Vec<_>>();
        assert_eq!(repair_tasks.len(), 1);
    }

    #[test]
    fn acceptance_repair_success_completes_objective() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(2),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        fixture.runner.fail_once();

        let result = fixture
            .orchestrator
            .supervise_objective(
                &objective_id,
                ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(
            fixture.store.get_objective(&objective_id).unwrap().status,
            ObjectiveStatus::Complete
        );
        assert_eq!(
            fixture
                .store
                .list_tasks(None)
                .unwrap()
                .iter()
                .filter(|task| task.title.starts_with("Repair objective acceptance:"))
                .count(),
            1
        );
    }

    #[test]
    fn objective_validation_unsafe_command_blocks_execution() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(2),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();
        fixture
            .store
            .force_objective_validation_command_for_test(
                &objective_id,
                "rm -rf target",
                ObjectiveValidationReviewStatus::Trusted,
            )
            .unwrap();

        let result = fixture
            .orchestrator
            .supervise_objective(
                &objective_id,
                ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Failed);
        assert_eq!(
            fixture.store.get_objective(&objective_id).unwrap().status,
            ObjectiveStatus::Blocked
        );
        assert_eq!(fixture.runner.outputs.lock().unwrap().len(), 0);
    }

    #[test]
    fn objective_validation_failures_use_distinct_artifact_paths() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(2),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        fixture.runner.fail_once();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        fixture.runner.fail_once();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();

        let paths = fixture
            .store
            .list_objective_artifacts(&objective_id)
            .unwrap()
            .into_iter()
            .filter(|artifact| artifact.kind == "objective_validation_failure")
            .map(|artifact| artifact.path)
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 2);
        assert_ne!(paths[0], paths[1]);
    }

    #[test]
    fn objective_validation_runner_error_persists_failure_event() {
        let fixture = Fixture::new(1);
        fixture.openai.push_text(valid_planner_json());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: RuntimeOptions::default(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(2),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();
        fixture.runner.error_once("runner unavailable");

        fixture
            .orchestrator
            .run_one_objective_cycle(
                &objective_id,
                &ObjectiveSuperviseOptions {
                    runtime: RuntimeOptions::default(),
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();

        let events = fixture.store.list_objective_events(&objective_id).unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "objective.validation_failed"
                    && event.message.contains("errored"))
        );
    }

    #[test]
    fn objective_validation_uses_runtime_repo_as_command_cwd() {
        let fixture = Fixture::new(1);
        let repo = fixture._temp.path().join("alternate-repo");
        fs::create_dir_all(&repo).unwrap();
        fixture.openai.push_text(valid_planner_json());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        let runtime = RuntimeOptions {
            repo: Some(repo.clone()),
            ..RuntimeOptions::default()
        };
        let start = fixture
            .orchestrator
            .start_objective(ObjectiveStartOptions {
                runtime: runtime.clone(),
                prompt: ObjectivePromptInput::Text("Build a Rust CLI".to_string()),
                supervise: false,
                planner_model: None,
                worker_model: None,
                ticket_model: None,
                max_worker_attempts: Some(2),
                max_cycles: None,
            })
            .unwrap();
        let objective_id =
            ObjectiveId::parse(start.data["objective_id"].as_str().unwrap()).unwrap();

        fixture
            .orchestrator
            .supervise_objective(
                &objective_id,
                ObjectiveSuperviseOptions {
                    runtime,
                    worker_model: None,
                    ticket_model: None,
                    max_worker_attempts: Some(2),
                    max_cycles: Some(8),
                },
            )
            .unwrap();

        let specs = fixture.runner.specs.lock().unwrap();
        assert_eq!(specs.last().unwrap().cwd, repo.to_string_lossy());
    }

    #[test]
    fn orchestrator_validation_failure_and_stuck_response_create_tickets() {
        let fixture = Fixture::new(1);
        let task = fixture.task();
        fixture.runner.fail_once();
        fixture.ollama.push_text(diff_response());
        let result = fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap();
        assert_eq!(result.exit.status, CommandStatus::Stuck);
        assert_eq!(
            fixture.store.list_tickets(Some(&task.id), None).unwrap()[0].blocked_on,
            "validation_failed"
        );

        let fixture = Fixture::new(1);
        let task = fixture.task();
        fixture
            .ollama
            .push_text("STUCK\nreason: need input\nquestion: Which API?");
        fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap();
        assert_eq!(
            fixture.store.list_tickets(Some(&task.id), None).unwrap()[0].blocked_on,
            "model_stuck"
        );
    }

    #[test]
    fn orchestrator_ticket_evidence_records_actual_failing_validation_command() {
        let fixture = Fixture::new(1);
        let task = fixture
            .orchestrator
            .create_task(
                "Fix".to_string(),
                "Make validation pass".to_string(),
                vec![
                    "cargo test --first".to_string(),
                    "cargo test --second".to_string(),
                ],
            )
            .unwrap();
        fixture.runner.fail_once();
        fixture.ollama.push_text(diff_response());

        let result = fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Stuck);
        let ticket = fixture.store.list_tickets(Some(&task.id), None).unwrap()[0].clone();
        let evidence: serde_json::Value = serde_json::from_str(&ticket.evidence_json).unwrap();
        assert_eq!(evidence["last_validation_command"], "cargo test --first");
        assert_eq!(evidence["last_validation_exit_code"], 101);
        assert!(evidence["last_validation_log_hash"].as_str().is_some());
        assert!(evidence["last_response_hash"].as_str().is_some());
        assert_eq!(
            ticket.failure_fingerprint,
            fingerprint(&[
                &normalize_fingerprint_text(&ticket.reason),
                "cargo test --first",
                "101",
                evidence["current_diff_hash"].as_str().unwrap(),
                evidence["last_validation_log_hash"].as_str().unwrap(),
                evidence["last_response_hash"].as_str().unwrap(),
            ])
        );
    }

    #[test]
    fn orchestrator_invalid_response_sticks_without_retrying_model_contract_failure() {
        let fixture = Fixture::new(2);
        let task = fixture.task();
        fixture.ollama.push_text("invalid");
        fixture.ollama.push_text(diff_response());
        let result = fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(2),
                    model: None,
                },
            )
            .unwrap();
        assert_eq!(result.exit.status, CommandStatus::Stuck);
        assert_eq!(fixture.ollama.requests().len(), 1);
        assert_eq!(
            fixture.store.list_tickets(Some(&task.id), None).unwrap()[0].blocked_on,
            "invalid_response"
        );

        let fixture = Fixture::new(1);
        let task = fixture.task();
        fixture.ollama.push_text("invalid");
        let result = fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(2),
                    model: None,
                },
            )
            .unwrap();
        assert_eq!(result.exit.status, CommandStatus::Stuck);
        assert_eq!(
            fixture.store.list_tickets(Some(&task.id), None).unwrap()[0].blocked_on,
            "invalid_response"
        );
    }

    #[test]
    fn orchestrator_provider_retries_only_retryable_errors() {
        let fixture = Fixture::new(1);
        let task = fixture.task();
        fixture
            .ollama
            .push_error(ProviderErrorKind::RateLimited, "retry later");
        fixture.ollama.push_text(diff_response());
        let result = fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(2),
                    model: None,
                },
            )
            .unwrap();
        assert_eq!(result.exit.status, CommandStatus::Complete);
        assert_eq!(fixture.ollama.requests().len(), 2);

        let fixture = Fixture::new(1);
        let task = fixture.task();
        fixture
            .ollama
            .push_error(ProviderErrorKind::InvalidJson, "bad provider json");
        fixture.ollama.push_text(diff_response());
        let result = fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(2),
                    model: None,
                },
            )
            .unwrap();
        assert_eq!(result.exit.status, CommandStatus::Stuck);
        assert_eq!(fixture.ollama.requests().len(), 1);
        assert_eq!(
            fixture.store.list_tickets(Some(&task.id), None).unwrap()[0].blocked_on,
            "provider_failure"
        );
    }

    #[test]
    fn orchestrator_patch_apply_failure_gets_one_retry_and_artifacts() {
        let fixture = Fixture::new(1);
        let task = fixture.task();
        fixture.workspace.apply_errors.lock().unwrap().extend([
            "second apply failure".to_string(),
            "first apply failure".to_string(),
        ]);
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());
        fixture.ollama.push_text(diff_response());

        let result = fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(3),
                    model: None,
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Stuck);
        assert_eq!(fixture.ollama.requests().len(), 2);
        let run = fixture
            .store
            .latest_run_for_task(&task.id)
            .unwrap()
            .unwrap();
        let attempts = fixture.store.list_attempts(&run.id).unwrap();
        assert_eq!(attempts.len(), 2);
        assert!(attempts.iter().all(|attempt| attempt.patch_path.is_some()));
        let artifacts = fixture.store.list_artifacts_for_run(&run.id).unwrap();
        assert!(artifacts.iter().any(|artifact| artifact.kind == "patch"));
        assert!(
            artifacts
                .iter()
                .any(|artifact| artifact.kind == "git_apply_stderr")
        );
        assert!(artifacts.iter().any(|artifact| artifact.kind == "manifest"));
    }

    #[test]
    fn orchestrator_patch_check_failure_records_check_stderr_artifact() {
        let fixture = Fixture::new(1);
        let task = fixture.task();
        fixture
            .workspace
            .check_errors
            .lock()
            .unwrap()
            .push("git apply --check failed: cannot apply".to_string());
        fixture.ollama.push_text(diff_response());

        let result = fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Stuck);
        let run = fixture
            .store
            .latest_run_for_task(&task.id)
            .unwrap()
            .unwrap();
        let artifacts = fixture.store.list_artifacts_for_run(&run.id).unwrap();
        assert!(
            artifacts
                .iter()
                .any(|artifact| artifact.kind == "git_apply_check_stderr")
        );
        assert!(
            !artifacts
                .iter()
                .any(|artifact| artifact.kind == "git_apply_stderr")
        );
    }

    #[test]
    fn orchestrator_openai_resolution_and_resume_consumption() {
        let fixture = Fixture::new(1);
        let task = fixture.task();
        fixture
            .ollama
            .push_text("STUCK\nreason: need advice\nquestion: What next?");
        fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap();
        let parent = fixture
            .store
            .latest_run_for_task(&task.id)
            .unwrap()
            .unwrap();
        let ticket = fixture.store.list_tickets(Some(&task.id), None).unwrap()[0].clone();
        fixture
            .openai
            .push_text_with_id("resp_1", "Use the failing assertion.");
        let resolved = fixture
            .orchestrator
            .resolve_ticket(
                &ticket.id,
                TicketResolveOptions {
                    runtime: Default::default(),
                    model: None,
                },
            )
            .unwrap();
        assert_eq!(resolved.exit.status, CommandStatus::Complete);
        assert_eq!(
            fixture.store.get_ticket(&ticket.id).unwrap().status,
            TicketStatus::Resolved
        );

        fixture.ollama.push_text(diff_response());
        let resumed = fixture
            .orchestrator
            .resume_task(
                &task.id,
                ResumeTaskOptions {
                    runtime: Default::default(),
                    ticket_id: Some(ticket.id.clone()),
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap();
        assert_eq!(resumed.exit.status, CommandStatus::Complete);
        let child = fixture
            .store
            .latest_run_for_task(&task.id)
            .unwrap()
            .unwrap();
        assert_eq!(child.parent_run_id, Some(parent.id));
        assert!(
            fixture
                .store
                .latest_unconsumed_resolution_for_ticket(&ticket.id)
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .ollama
                .requests()
                .last()
                .unwrap()
                .input
                .contains("Use the failing assertion.")
        );
    }

    #[test]
    fn orchestrator_resume_checks_escalation_before_running_or_consuming() {
        let mut fixture = Fixture::new(1);
        fixture
            .orchestrator
            .config
            .orchestrator
            .max_escalation_cycles = 0;
        let task = fixture.task();
        fixture
            .ollama
            .push_text("STUCK\nreason: need advice\nquestion: What next?");
        fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap();
        let ticket = fixture.store.list_tickets(Some(&task.id), None).unwrap()[0].clone();
        fixture.openai.push_text("Use the failing assertion.");
        fixture
            .orchestrator
            .resolve_ticket(
                &ticket.id,
                TicketResolveOptions {
                    runtime: Default::default(),
                    model: None,
                },
            )
            .unwrap();

        let error = fixture
            .orchestrator
            .resume_task(
                &task.id,
                ResumeTaskOptions {
                    runtime: Default::default(),
                    ticket_id: Some(ticket.id.clone()),
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap_err();

        assert!(error.to_string().contains("max escalation cycles exceeded"));
        assert_eq!(
            fixture.store.get_task(&task.id).unwrap().status,
            TaskStatus::Stuck
        );
        assert!(
            fixture
                .store
                .latest_unconsumed_resolution_for_ticket(&ticket.id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn orchestrator_resume_consumes_resolution_only_after_successful_ollama_send() {
        let fixture = Fixture::new(1);
        let task = fixture.task();
        fixture
            .ollama
            .push_text("STUCK\nreason: need advice\nquestion: What next?");
        fixture
            .orchestrator
            .run_task(
                &task.id,
                TaskRunOptions {
                    runtime: Default::default(),
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap();
        let ticket = fixture.store.list_tickets(Some(&task.id), None).unwrap()[0].clone();
        fixture.openai.push_text(
            "OPENAI_API_KEY=sk-proj-abcdefABCDEF1234567890abcdefABCDEF\nUse the redacted key.",
        );
        fixture
            .orchestrator
            .resolve_ticket(
                &ticket.id,
                TicketResolveOptions {
                    runtime: Default::default(),
                    model: None,
                },
            )
            .unwrap();
        fixture
            .ollama
            .push_error(ProviderErrorKind::InvalidJson, "bad provider json");

        let result = fixture
            .orchestrator
            .resume_task(
                &task.id,
                ResumeTaskOptions {
                    runtime: Default::default(),
                    ticket_id: Some(ticket.id.clone()),
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap();

        assert_eq!(result.exit.status, CommandStatus::Stuck);
        assert!(
            fixture
                .store
                .latest_unconsumed_resolution_for_ticket(&ticket.id)
                .unwrap()
                .is_some()
        );
        let requests = fixture.ollama.requests();
        let request = requests.last().unwrap();
        assert!(
            !request
                .input
                .contains("sk-proj-abcdefABCDEF1234567890abcdefABCDEF")
        );
        assert!(request.input.contains("[REDACTED"));
    }

    #[test]
    fn orchestrator_bare_resume_uses_latest_stuck_run_resolution_only() {
        let fixture = Fixture::new(1);
        let task = fixture.task();
        let owner = owner(&task.id);
        fixture.store.acquire_task_lease(&task.id, &owner).unwrap();
        fixture
            .store
            .transition_task(&task.id, TaskStatus::Ready, TaskStatus::Running, &owner)
            .unwrap();

        let run1 = Run {
            id: new_id(RunId::PREFIX, RunId::parse).unwrap(),
            task_id: task.id.clone(),
            parent_run_id: None,
            status: RunStatus::Stuck,
            repo_root: task.repo_root.clone(),
            base_ref: task.base_ref.clone(),
            base_commit: "base".to_string(),
            dirty_state_summary: None,
            current_phase: Some("stuck".to_string()),
            escalation_cycle: 0,
            started_at: now(),
            finished_at: Some(now()),
            final_diff_path: None,
            last_error: Some("first".to_string()),
        };
        fixture.store.insert_run(run1.clone(), &owner).unwrap();
        let ticket1 = Ticket {
            id: new_id(TicketId::PREFIX, TicketId::parse).unwrap(),
            task_id: task.id.clone(),
            run_id: run1.id.clone(),
            status: TicketStatus::Open,
            blocked_on: "validation_failed".to_string(),
            question: "first?".to_string(),
            reason: "first".to_string(),
            evidence_json: "{}".to_string(),
            failure_fingerprint: "first".to_string(),
            created_at: now(),
            resolved_at: None,
        };
        fixture
            .store
            .insert_ticket(ticket1.clone(), &owner)
            .unwrap();
        fixture
            .store
            .transition_ticket(
                &ticket1.id,
                TicketStatus::Open,
                TicketStatus::Resolving,
                &owner,
            )
            .unwrap();
        let resolution_path = fixture
            .orchestrator
            .write_artifact(
                &task.id,
                Some(&run1.id),
                None,
                Some(&ticket1.id),
                "resolution.md",
                "old resolution",
            )
            .unwrap();
        fixture
            .store
            .insert_ticket_resolution(
                TicketResolution {
                    id: new_id(TicketResolutionId::PREFIX, TicketResolutionId::parse).unwrap(),
                    ticket_id: ticket1.id.clone(),
                    provider: "fake-openai".to_string(),
                    model: "fake".to_string(),
                    response_id: None,
                    resolution_path,
                    consumed_at: None,
                    created_at: now(),
                },
                &owner,
            )
            .unwrap();

        fixture
            .store
            .transition_task(&task.id, TaskStatus::Running, TaskStatus::Stuck, &owner)
            .unwrap();
        fixture
            .store
            .transition_task(&task.id, TaskStatus::Stuck, TaskStatus::Running, &owner)
            .unwrap();
        let run2 = Run {
            id: new_id(RunId::PREFIX, RunId::parse).unwrap(),
            task_id: task.id.clone(),
            parent_run_id: Some(run1.id.clone()),
            status: RunStatus::Stuck,
            repo_root: task.repo_root.clone(),
            base_ref: task.base_ref.clone(),
            base_commit: "base".to_string(),
            dirty_state_summary: None,
            current_phase: Some("stuck".to_string()),
            escalation_cycle: 1,
            started_at: now(),
            finished_at: Some(now()),
            final_diff_path: None,
            last_error: Some("second".to_string()),
        };
        fixture.store.insert_run(run2.clone(), &owner).unwrap();
        let ticket2 = Ticket {
            id: new_id(TicketId::PREFIX, TicketId::parse).unwrap(),
            task_id: task.id.clone(),
            run_id: run2.id,
            status: TicketStatus::Open,
            blocked_on: "validation_failed".to_string(),
            question: "second?".to_string(),
            reason: "second".to_string(),
            evidence_json: "{}".to_string(),
            failure_fingerprint: "second".to_string(),
            created_at: now(),
            resolved_at: None,
        };
        fixture.store.insert_ticket(ticket2, &owner).unwrap();
        fixture
            .store
            .transition_task(&task.id, TaskStatus::Running, TaskStatus::Stuck, &owner)
            .unwrap();
        fixture.store.release_task_lease(&task.id, &owner).unwrap();

        let error = fixture
            .orchestrator
            .resume_task(
                &task.id,
                ResumeTaskOptions {
                    runtime: Default::default(),
                    ticket_id: None,
                    max_attempts: Some(1),
                    model: None,
                },
            )
            .unwrap_err();

        assert!(matches!(error, HarnessError::NotFound { .. }));
        assert!(
            fixture
                .store
                .latest_unconsumed_resolution_for_ticket(&ticket1.id)
                .unwrap()
                .is_some()
        );
    }

    fn diff_response() -> &'static str {
        "```diff\ndiff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n```"
    }

    fn valid_planner_json() -> String {
        serde_json::json!({
            "schema_version": 1,
            "objective": {
                "title": "Build Rust CLI",
                "summary": "Create a Rust CLI implementation.",
                "acceptance_criteria": ["cargo test passes"],
                "validation_commands": ["cargo test"]
            },
            "tasks": [
                {
                    "task_key": "inspect_cli",
                    "title": "Inspect CLI",
                    "goal": "Understand the CLI command surface.",
                    "validation": ["cargo test inspect_cli"],
                    "depends_on": [],
                    "owned_paths": ["src"],
                    "parallel_group": "discovery"
                },
                {
                    "task_key": "implement_cli",
                    "title": "Implement CLI",
                    "goal": "Implement the Rust CLI.",
                    "validation": ["cargo test implement_cli"],
                    "depends_on": ["inspect_cli"],
                    "owned_paths": ["src"],
                    "parallel_group": "implementation"
                }
            ],
            "risks": [],
            "final_verification": ["cargo test"]
        })
        .to_string()
    }

    fn unsafe_validation_planner_json() -> String {
        let mut value: serde_json::Value = serde_json::from_str(&valid_planner_json()).unwrap();
        value["tasks"][0]["validation"] = serde_json::json!(["rm -rf target"]);
        value.to_string()
    }

    fn valid_resolver_json() -> String {
        serde_json::json!({
            "schema_version": 1,
            "diagnosis": "The local worker needs to add the missing command implementation.",
            "recommended_steps": [
                "Add the missing command branch using the existing CLI patterns.",
                "Keep the validation command unchanged and rerun it after the change."
            ],
            "constraints": ["Do not change unrelated command behavior."],
            "validation_focus": ["cargo test inspect_cli"]
        })
        .to_string()
    }

    fn patch_like_resolver_json() -> String {
        serde_json::json!({
            "schema_version": 1,
            "diagnosis": "The response must be rejected.",
            "recommended_steps": ["diff --git a/src/lib.rs b/src/lib.rs"],
            "constraints": [],
            "validation_focus": []
        })
        .to_string()
    }
}
