pub mod supervisor;

use crate::domain::{HarnessConfig, Task, TaskId, Ticket, TicketId};
use crate::orchestrator::RunOrchestrator;
use crate::providers::{
    ModelProvider, ModelRequest, ModelResponse, OllamaProvider, OpenAiCompatibleProvider,
    ProviderError, ProviderErrorKind, ProviderFuture,
};
use crate::runtime::{
    CommandResult, OutputSink, ResumeTaskOptions, SuperviseCreateOptions, SuperviseTaskOptions,
    TaskRunOptions, TicketResolveOptions,
};
use crate::state::{SqliteTaskStore, TaskStore};
use crate::workspace::{CommandRunner, GitWorkspaceManager, WorkspaceManager};
use crate::{HarnessError, HarnessResult};
use std::path::PathBuf;
use std::sync::Arc;

pub use supervisor::{
    SUPERVISOR_PLACEHOLDER_MESSAGE, SupervisorService, SupervisorStateStore, SupervisorStateView,
    SupervisorTaskState, supervise_placeholder_result,
};

pub trait HarnessService {
    fn create_task(
        &self,
        title: String,
        goal: String,
        validation_commands: Vec<String>,
    ) -> HarnessResult<Task>;

    fn list_tasks(&self) -> HarnessResult<Vec<Task>>;
    fn get_task(&self, task_id: &TaskId) -> HarnessResult<Task>;
    fn run_task(&self, task_id: &TaskId, options: TaskRunOptions) -> HarnessResult<CommandResult>;
    fn list_tickets(&self) -> HarnessResult<Vec<Ticket>>;
    fn get_ticket(&self, ticket_id: &TicketId) -> HarnessResult<Ticket>;
    fn resolve_ticket(
        &self,
        ticket_id: &TicketId,
        options: TicketResolveOptions,
    ) -> HarnessResult<CommandResult>;
    fn resume_task(
        &self,
        task_id: &TaskId,
        options: ResumeTaskOptions,
    ) -> HarnessResult<CommandResult>;

    fn supervise_task(
        &self,
        task_id: &TaskId,
        _options: SuperviseTaskOptions,
    ) -> HarnessResult<CommandResult> {
        Ok(supervise_placeholder_result(Some(task_id)))
    }

    fn supervise_task_streaming(
        &self,
        task_id: &TaskId,
        options: SuperviseTaskOptions,
        sink: &mut dyn OutputSink,
    ) -> HarnessResult<CommandResult> {
        let result = self.supervise_task(task_id, options)?;
        for event in &result.events {
            sink.event(event)?;
        }
        Ok(result)
    }

    fn create_and_supervise_task(
        &self,
        _options: SuperviseCreateOptions,
    ) -> HarnessResult<CommandResult> {
        Ok(supervise_placeholder_result(None))
    }

    fn create_and_supervise_task_streaming(
        &self,
        options: SuperviseCreateOptions,
        sink: &mut dyn OutputSink,
    ) -> HarnessResult<CommandResult> {
        let result = self.create_and_supervise_task(options)?;
        for event in &result.events {
            sink.event(event)?;
        }
        Ok(result)
    }
}

pub struct DefaultHarnessService {
    orchestrator: RunOrchestrator,
}

impl DefaultHarnessService {
    pub fn new(orchestrator: RunOrchestrator) -> Self {
        Self { orchestrator }
    }

    pub fn from_parts(
        config: HarnessConfig,
        store: Arc<dyn TaskStore>,
        workspace: Arc<dyn WorkspaceManager>,
        command_runner: Arc<dyn CommandRunner>,
        ollama: Arc<dyn ModelProvider>,
        openai: Arc<dyn ModelProvider>,
    ) -> Self {
        Self::new(RunOrchestrator::new(
            config,
            store,
            workspace,
            command_runner,
            ollama,
            openai,
        ))
    }

    pub fn for_current_dir(config: HarnessConfig) -> HarnessResult<Self> {
        let state_path = PathBuf::from(&config.workspace.state_dir).join("state.sqlite");
        let store = Arc::new(SqliteTaskStore::open(state_path)?);
        let workspace = Arc::new(GitWorkspaceManager::for_current_dir()?);
        let command_runner: Arc<dyn CommandRunner> = workspace.clone();
        let ollama = Arc::new(OllamaProvider::new(&config.providers.ollama));
        let openai = Arc::new(
            OpenAiCompatibleProvider::from_env(&config.providers.openai)
                .map_err(|error| HarnessError::External(error.message))?,
        );
        Ok(Self::from_parts(
            config,
            store,
            workspace,
            command_runner,
            ollama,
            openai,
        ))
    }

    pub fn from_loaded_config(loaded: crate::config::LoadedConfig) -> HarnessResult<Self> {
        let config = loaded.config;
        let store = Arc::new(SqliteTaskStore::open(&loaded.paths.state_file)?);
        let workspace = Arc::new(GitWorkspaceManager::new(
            &loaded.paths.repo_root,
            &loaded.paths.worktree_root,
        )?);
        let command_runner: Arc<dyn CommandRunner> = workspace.clone();
        let ollama = Arc::new(OllamaProvider::new(&config.providers.ollama));
        let openai: Arc<dyn ModelProvider> =
            match OpenAiCompatibleProvider::from_env(&config.providers.openai) {
                Ok(provider) => Arc::new(provider),
                Err(error) => Arc::new(MissingProvider { error }),
            };
        Ok(Self::from_parts(
            config,
            store,
            workspace,
            command_runner,
            ollama,
            openai,
        ))
    }
}

#[derive(Debug)]
struct MissingProvider {
    error: ProviderError,
}

impl ModelProvider for MissingProvider {
    fn complete<'a>(&'a self, _request: ModelRequest) -> ProviderFuture<'a, ModelResponse> {
        Box::pin(async move {
            Err(ProviderError::new(
                ProviderErrorKind::AuthFailed,
                self.error.message.clone(),
            ))
        })
    }
}

impl HarnessService for DefaultHarnessService {
    fn create_task(
        &self,
        title: String,
        goal: String,
        validation_commands: Vec<String>,
    ) -> HarnessResult<Task> {
        self.orchestrator
            .create_task(title, goal, validation_commands)
    }

    fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
        self.orchestrator.list_tasks()
    }

    fn get_task(&self, task_id: &TaskId) -> HarnessResult<Task> {
        self.orchestrator.get_task(task_id)
    }

    fn run_task(&self, task_id: &TaskId, options: TaskRunOptions) -> HarnessResult<CommandResult> {
        self.orchestrator.run_task(task_id, options)
    }

    fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
        self.orchestrator.list_tickets()
    }

    fn get_ticket(&self, ticket_id: &TicketId) -> HarnessResult<Ticket> {
        self.orchestrator.get_ticket(ticket_id)
    }

    fn resolve_ticket(
        &self,
        ticket_id: &TicketId,
        options: TicketResolveOptions,
    ) -> HarnessResult<CommandResult> {
        self.orchestrator.resolve_ticket(ticket_id, options)
    }

    fn resume_task(
        &self,
        task_id: &TaskId,
        options: ResumeTaskOptions,
    ) -> HarnessResult<CommandResult> {
        self.orchestrator.resume_task(task_id, options)
    }

    fn supervise_task(
        &self,
        task_id: &TaskId,
        options: SuperviseTaskOptions,
    ) -> HarnessResult<CommandResult> {
        self.orchestrator.supervise_task(task_id, options)
    }

    fn supervise_task_streaming(
        &self,
        task_id: &TaskId,
        options: SuperviseTaskOptions,
        sink: &mut dyn OutputSink,
    ) -> HarnessResult<CommandResult> {
        self.orchestrator
            .supervise_task_with_progress_sink(task_id, options, sink)
    }

    fn create_and_supervise_task(
        &self,
        options: SuperviseCreateOptions,
    ) -> HarnessResult<CommandResult> {
        self.orchestrator.create_and_supervise_task(options)
    }

    fn create_and_supervise_task_streaming(
        &self,
        options: SuperviseCreateOptions,
        sink: &mut dyn OutputSink,
    ) -> HarnessResult<CommandResult> {
        self.orchestrator
            .create_and_supervise_task_with_progress_sink(options, sink)
    }
}

impl SupervisorService for DefaultHarnessService {
    fn supervise_task(
        &self,
        task_id: &TaskId,
        options: SuperviseTaskOptions,
    ) -> HarnessResult<CommandResult> {
        HarnessService::supervise_task(self, task_id, options)
    }

    fn create_and_supervise_task(
        &self,
        options: SuperviseCreateOptions,
    ) -> HarnessResult<CommandResult> {
        HarnessService::create_and_supervise_task(self, options)
    }
}
