use crate::config::{self, ConfigPaths, LoadedConfig};
use crate::domain::HarnessConfig;
use crate::error::{HarnessError, HarnessResult};
use crate::providers::{
    ModelProvider, ModelRequest, OllamaProvider, OpenAiCompatibleProvider, ProviderError,
    ProviderFuture,
};
use crate::security::{
    DefaultProviderUrlPolicy, DefaultRedactor, ProviderUrlPolicy, Redactor,
    check_private_permissions,
};
use crate::state::SqliteTaskStore;
use crate::workspace::{GitWorkspaceManager, WorkspaceManager};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::task::{Context, Poll};
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorMode {
    Offline,
    ProvidersLocal,
    ProvidersAll,
}

impl DoctorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::ProvidersLocal => "providers_local",
            Self::ProvidersAll => "providers_all",
        }
    }

    fn provider_names(self) -> &'static [&'static str] {
        match self {
            Self::Offline => &[],
            Self::ProvidersLocal => &["ollama"],
            Self::ProvidersAll => &["ollama", "openai-compatible"],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorOptions {
    pub repo: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub mode: DoctorMode,
    pub deep: bool,
}

impl DoctorOptions {
    pub fn offline(repo: Option<PathBuf>, state_dir: Option<PathBuf>) -> Self {
        Self {
            repo,
            state_dir,
            mode: DoctorMode::Offline,
            deep: false,
        }
    }

    pub fn from_cli(
        repo: Option<PathBuf>,
        state_dir: Option<PathBuf>,
        offline: bool,
        providers: Option<&str>,
        deep: bool,
    ) -> Self {
        let mode = if offline {
            DoctorMode::Offline
        } else {
            match providers {
                Some("all") => DoctorMode::ProvidersAll,
                Some("local") => DoctorMode::ProvidersLocal,
                Some(_) | None if deep => DoctorMode::ProvidersLocal,
                Some(_) | None => DoctorMode::Offline,
            }
        };

        Self {
            repo,
            state_dir,
            mode,
            deep: deep && mode != DoctorMode::Offline,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub mode: DoctorMode,
    pub deep: bool,
    pub repo_root: Option<String>,
    pub passed: bool,
    pub summary: DoctorSummary,
    pub checks: Vec<DiagnosticCheck>,
}

impl DoctorReport {
    pub fn has_failures(&self) -> bool {
        !self.passed
    }

    pub fn message(&self) -> String {
        if self.passed {
            format!(
                "doctor {} passed: {} passed, {} warning(s), {} skipped",
                self.mode.as_str(),
                self.summary.passed,
                self.summary.warnings,
                self.summary.skipped
            )
        } else {
            format!(
                "doctor {} failed: {} failed, {} warning(s), {} passed",
                self.mode.as_str(),
                self.summary.failed,
                self.summary.warnings,
                self.summary.passed
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorSummary {
    pub passed: usize,
    pub warnings: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticCheck {
    pub id: String,
    pub label: String,
    pub status: DiagnosticStatus,
    pub message: String,
    pub details: BTreeMap<String, String>,
}

impl DiagnosticCheck {
    pub fn pass(
        id: impl Into<String>,
        label: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(id, label, DiagnosticStatus::Pass, message)
    }

    pub fn warn(
        id: impl Into<String>,
        label: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(id, label, DiagnosticStatus::Warn, message)
    }

    pub fn fail(
        id: impl Into<String>,
        label: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(id, label, DiagnosticStatus::Fail, message)
    }

    pub fn skipped(
        id: impl Into<String>,
        label: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(id, label, DiagnosticStatus::Skipped, message)
    }

    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        status: DiagnosticStatus,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            status,
            message: message.into(),
            details: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStatus {
    Pass,
    Warn,
    Fail,
    Skipped,
}

impl DiagnosticStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skipped => "skipped",
        }
    }
}

pub fn run_doctor(options: DoctorOptions) -> DoctorReport {
    run_doctor_with_env(options, std::env::vars())
}

pub fn run_doctor_with_env<I, K, V>(options: DoctorOptions, env: I) -> DoctorReport
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let env = env
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect::<BTreeMap<_, _>>();
    let mut checks = Vec::new();
    let redactor = DefaultRedactor::new();

    let repo_root = match config::discover_repo_root(options.repo.as_deref()) {
        Ok(repo_root) => {
            checks.push(
                DiagnosticCheck::pass(
                    "repo.discovery",
                    "Repository discovery",
                    "git repository found",
                )
                .with_detail("repo_root", path_string_lossy(&repo_root)),
            );
            repo_root
        }
        Err(error) => {
            checks.push(DiagnosticCheck::fail(
                "repo.discovery",
                "Repository discovery",
                redact_error(&redactor, &error.to_string()),
            ));
            return build_report(options.mode, options.deep, None, checks);
        }
    };

    check_harness_dir(&repo_root, &mut checks);

    let loaded =
        match load_config_for_doctor(options.repo.as_deref(), options.state_dir.as_deref(), &env) {
            Ok(loaded) => {
                checks.push(
                    DiagnosticCheck::pass("config.load", "Config", "configuration loaded")
                        .with_detail("config_file", path_string_lossy(&loaded.paths.config_file)),
                );
                loaded
            }
            Err(error) => {
                checks.push(DiagnosticCheck::fail(
                    "config.load",
                    "Config",
                    redact_error(&redactor, &error.to_string()),
                ));
                return build_report(
                    options.mode,
                    options.deep,
                    Some(path_string_lossy(&repo_root)),
                    checks,
                );
            }
        };

    check_state(&loaded.paths, &mut checks);
    check_workspace(&repo_root, &loaded.paths, &mut checks);
    check_security_policy(&loaded.config, &mut checks);

    if options.mode == DoctorMode::Offline {
        checks.push(DiagnosticCheck::skipped(
            "providers.network",
            "Provider network",
            "offline mode skips provider network checks",
        ));
    } else {
        check_providers(
            &loaded.config,
            options.mode,
            options.deep,
            &env,
            &redactor,
            &mut checks,
        );
    }

    build_report(
        options.mode,
        options.deep,
        Some(path_string_lossy(&repo_root)),
        checks,
    )
}

fn build_report(
    mode: DoctorMode,
    deep: bool,
    repo_root: Option<String>,
    checks: Vec<DiagnosticCheck>,
) -> DoctorReport {
    let summary = DoctorSummary {
        passed: checks
            .iter()
            .filter(|check| check.status == DiagnosticStatus::Pass)
            .count(),
        warnings: checks
            .iter()
            .filter(|check| check.status == DiagnosticStatus::Warn)
            .count(),
        failed: checks
            .iter()
            .filter(|check| check.status == DiagnosticStatus::Fail)
            .count(),
        skipped: checks
            .iter()
            .filter(|check| check.status == DiagnosticStatus::Skipped)
            .count(),
    };

    DoctorReport {
        schema_version: 1,
        mode,
        deep,
        repo_root,
        passed: summary.failed == 0,
        summary,
        checks,
    }
}

fn load_config_for_doctor(
    repo: Option<&Path>,
    state_dir: Option<&Path>,
    env: &BTreeMap<String, String>,
) -> HarnessResult<LoadedConfig> {
    let mut loaded = config::load_config_with_env(repo, env.iter())?;
    if let Some(state_dir) = state_dir {
        let state_dir = state_dir.to_str().ok_or_else(|| {
            HarnessError::InvalidConfig(format!("non-UTF-8 state dir {}", state_dir.display()))
        })?;
        let normalized = config::normalize_configured_path(&loaded.paths.repo_root, state_dir)?;
        loaded.config.workspace.state_dir = path_to_string(&normalized)?;
        loaded.paths = ConfigPaths::for_repo(&loaded.paths.repo_root, &loaded.config)?;
    }
    Ok(loaded)
}

fn check_harness_dir(repo_root: &Path, checks: &mut Vec<DiagnosticCheck>) {
    let harness_dir = repo_root.join(config::HARNESS_DIR_NAME);
    match fs::metadata(&harness_dir) {
        Ok(metadata) if metadata.is_dir() => checks.push(
            DiagnosticCheck::pass(
                "harness.dir",
                ".harness directory",
                ".harness directory exists",
            )
            .with_detail("path", path_string_lossy(&harness_dir)),
        ),
        Ok(_) => checks.push(DiagnosticCheck::fail(
            "harness.dir",
            ".harness directory",
            format!("{} exists but is not a directory", harness_dir.display()),
        )),
        Err(error) => checks.push(DiagnosticCheck::fail(
            "harness.dir",
            ".harness directory",
            format!("{} is not ready: {error}", harness_dir.display()),
        )),
    }
}

fn check_state(paths: &ConfigPaths, checks: &mut Vec<DiagnosticCheck>) {
    match SqliteTaskStore::open(&paths.state_file) {
        Ok(store) => match store.pragma_value("journal_mode") {
            Ok(journal_mode) => checks.push(
                DiagnosticCheck::pass("state.sqlite", "SQLite state", "state database opened")
                    .with_detail("state_file", path_string_lossy(&paths.state_file))
                    .with_detail("journal_mode", journal_mode),
            ),
            Err(error) => checks.push(DiagnosticCheck::fail(
                "state.sqlite",
                "SQLite state",
                error.to_string(),
            )),
        },
        Err(error) => checks.push(DiagnosticCheck::fail(
            "state.sqlite",
            "SQLite state",
            error.to_string(),
        )),
    }

    for (id, label, path, directory) in [
        (
            "state.permissions.dir",
            "State directory permissions",
            &paths.state_dir,
            true,
        ),
        (
            "state.permissions.file",
            "State file permissions",
            &paths.state_file,
            false,
        ),
    ] {
        match check_private_permissions(path, directory) {
            crate::security::PermissionCheck::Secure => checks.push(
                DiagnosticCheck::pass(id, label, "permissions are private")
                    .with_detail("path", path_string_lossy(path)),
            ),
            crate::security::PermissionCheck::Warning(message) => {
                checks.push(DiagnosticCheck::warn(id, label, message))
            }
            crate::security::PermissionCheck::Failure(message) => {
                checks.push(DiagnosticCheck::fail(id, label, message))
            }
        }
    }
}

fn check_workspace(repo_root: &Path, paths: &ConfigPaths, checks: &mut Vec<DiagnosticCheck>) {
    match command_output("git", ["--version"], repo_root) {
        Ok(version) => checks.push(
            DiagnosticCheck::pass("git.available", "Git", "git executable is available")
                .with_detail("version", version.trim()),
        ),
        Err(error) => checks.push(DiagnosticCheck::fail("git.available", "Git", error)),
    }

    match command_output("git", ["worktree", "list", "--porcelain"], repo_root) {
        Ok(_) => checks.push(DiagnosticCheck::pass(
            "git.worktree",
            "Git worktree",
            "git worktree support is available",
        )),
        Err(error) => checks.push(DiagnosticCheck::fail("git.worktree", "Git worktree", error)),
    }

    match GitWorkspaceManager::new(repo_root, &paths.worktree_root) {
        Ok(manager) => {
            let repo = path_string_lossy(repo_root);
            match manager.resolve_base_commit(&repo, Some("HEAD")) {
                Ok(commit) => checks.push(
                    DiagnosticCheck::pass("workspace.head", "Workspace HEAD", "HEAD resolves")
                        .with_detail("commit", commit),
                ),
                Err(error) => checks.push(DiagnosticCheck::fail(
                    "workspace.head",
                    "Workspace HEAD",
                    error.to_string(),
                )),
            }
            match manager.source_is_dirty(&repo) {
                Ok(false) => checks.push(DiagnosticCheck::pass(
                    "workspace.clean",
                    "Workspace cleanliness",
                    "source checkout is clean",
                )),
                Ok(true) => checks.push(DiagnosticCheck::warn(
                    "workspace.clean",
                    "Workspace cleanliness",
                    "source checkout has uncommitted changes",
                )),
                Err(error) => checks.push(DiagnosticCheck::fail(
                    "workspace.clean",
                    "Workspace cleanliness",
                    error.to_string(),
                )),
            }
        }
        Err(error) => checks.push(DiagnosticCheck::fail(
            "workspace.manager",
            "Workspace manager",
            error.to_string(),
        )),
    }
}

fn check_security_policy(config: &HarnessConfig, checks: &mut Vec<DiagnosticCheck>) {
    let policy = DefaultProviderUrlPolicy::new();
    match policy.validate_credentialed_url(
        &config.providers.openai.base_url,
        config.providers.openai.allow_untrusted_provider_url,
    ) {
        Ok(()) => checks.push(DiagnosticCheck::pass(
            "security.openai_url",
            "OpenAI-compatible URL policy",
            "credentialed provider URL is allowed",
        )),
        Err(error) => checks.push(DiagnosticCheck::fail(
            "security.openai_url",
            "OpenAI-compatible URL policy",
            error.to_string(),
        )),
    }
}

fn check_providers(
    config: &HarnessConfig,
    mode: DoctorMode,
    deep: bool,
    env: &BTreeMap<String, String>,
    redactor: &DefaultRedactor,
    checks: &mut Vec<DiagnosticCheck>,
) {
    for provider in mode.provider_names() {
        match *provider {
            "ollama" => check_ollama(config, deep, redactor, checks),
            "openai-compatible" => check_openai(config, deep, env, redactor, checks),
            _ => {}
        }
    }
}

fn check_ollama(
    config: &HarnessConfig,
    deep: bool,
    redactor: &DefaultRedactor,
    checks: &mut Vec<DiagnosticCheck>,
) {
    let provider = OllamaProvider::new(&config.providers.ollama);
    let model = &config.providers.ollama.default_model;
    match provider.list_models() {
        Ok(models) if models.iter().any(|listed| listed == model) => checks.push(
            DiagnosticCheck::pass(
                "provider.ollama.models",
                "Ollama models",
                "default model is listed",
            )
            .with_detail("base_url", config.providers.ollama.base_url.clone())
            .with_detail("model", model.clone()),
        ),
        Ok(models) => checks.push(
            DiagnosticCheck::fail(
                "provider.ollama.models",
                "Ollama models",
                "default model was not listed by provider",
            )
            .with_detail("base_url", config.providers.ollama.base_url.clone())
            .with_detail("model", model.clone())
            .with_detail("listed_models", models.join(",")),
        ),
        Err(error) => checks.push(provider_error_check(
            "provider.ollama.models",
            "Ollama models",
            error,
            redactor,
        )),
    }

    if deep {
        let request = tiny_request(model);
        match block_on_provider(provider.complete(request)) {
            Ok(response) => checks.push(
                DiagnosticCheck::pass(
                    "provider.ollama.deep",
                    "Ollama tiny generation",
                    "tiny generation completed",
                )
                .with_detail("model", response.model)
                .with_detail("provider", response.provider),
            ),
            Err(error) => checks.push(provider_error_check(
                "provider.ollama.deep",
                "Ollama tiny generation",
                error,
                redactor,
            )),
        }
    }
}

fn check_openai(
    config: &HarnessConfig,
    deep: bool,
    env: &BTreeMap<String, String>,
    redactor: &DefaultRedactor,
    checks: &mut Vec<DiagnosticCheck>,
) {
    let Some(api_key) = env
        .get(&config.providers.openai.api_key_env)
        .or_else(|| env.get(&config.providers.openai.fallback_api_key_env))
    else {
        checks.push(DiagnosticCheck::fail(
            "provider.openai.auth",
            "OpenAI-compatible credentials",
            format!(
                "missing API key env var {} or {}",
                config.providers.openai.api_key_env, config.providers.openai.fallback_api_key_env
            ),
        ));
        return;
    };

    let provider = match OpenAiCompatibleProvider::new(&config.providers.openai, api_key) {
        Ok(provider) => provider,
        Err(error) => {
            checks.push(provider_error_check(
                "provider.openai.construct",
                "OpenAI-compatible provider",
                error,
                redactor,
            ));
            return;
        }
    };

    let model = &config.providers.openai.default_model;
    match provider.list_models() {
        Ok(models) if models.iter().any(|listed| listed == model) => checks.push(
            DiagnosticCheck::pass(
                "provider.openai.models",
                "OpenAI-compatible models",
                "default model is listed",
            )
            .with_detail("base_url", config.providers.openai.base_url.clone())
            .with_detail("model", model.clone()),
        ),
        Ok(models) => checks.push(
            DiagnosticCheck::fail(
                "provider.openai.models",
                "OpenAI-compatible models",
                "default model was not listed by provider",
            )
            .with_detail("base_url", config.providers.openai.base_url.clone())
            .with_detail("model", model.clone())
            .with_detail("listed_models", models.join(",")),
        ),
        Err(error) => checks.push(provider_error_check(
            "provider.openai.models",
            "OpenAI-compatible models",
            error,
            redactor,
        )),
    }

    if deep {
        let request = tiny_request(model);
        match block_on_provider(provider.complete(request)) {
            Ok(response) => checks.push(
                DiagnosticCheck::pass(
                    "provider.openai.deep",
                    "OpenAI-compatible tiny generation",
                    "tiny generation completed",
                )
                .with_detail("model", response.model)
                .with_detail("provider", response.provider),
            ),
            Err(error) => checks.push(provider_error_check(
                "provider.openai.deep",
                "OpenAI-compatible tiny generation",
                error,
                redactor,
            )),
        }
    }
}

fn tiny_request(model: &str) -> ModelRequest {
    ModelRequest {
        model: model.to_string(),
        system: Some("Diagnostic readiness check. Reply briefly.".to_string()),
        input: "Reply with ok.".to_string(),
        temperature: Some(0.0),
        max_output_tokens: Some(16),
        metadata: BTreeMap::from([("diagnostic".to_string(), "doctor".to_string())]),
    }
}

fn provider_error_check(
    id: impl Into<String>,
    label: impl Into<String>,
    error: ProviderError,
    redactor: &DefaultRedactor,
) -> DiagnosticCheck {
    DiagnosticCheck::fail(id, label, redact_error(redactor, &error.message))
        .with_detail("error_kind", error.kind.as_str())
}

fn block_on_provider<T>(future: ProviderFuture<'_, T>) -> Result<T, ProviderError> {
    let waker = std::task::Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut future = future;
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::yield_now(),
        }
    }
}

fn command_output<const N: usize>(
    program: &str,
    args: [&str; N],
    cwd: &Path,
) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("spawn {program}: {err}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn redact_error(redactor: &DefaultRedactor, message: &str) -> String {
    redactor.redact(message).text
}

fn path_to_string(path: &Path) -> HarnessResult<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| HarnessError::InvalidConfig(format!("non-UTF-8 path {}", path.display())))
}

fn path_string_lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{FakeHttpResponse, FakeHttpRoute, FakeHttpServer};
    use std::process::Command;

    #[test]
    fn offline_doctor_passes_for_initialized_repo_without_network() {
        let temp = tempfile::tempdir().unwrap();
        let repo = init_test_repo(temp.path().join("repo"));
        config::init_repo(Some(&repo)).unwrap();

        let server = FakeHttpServer::ollama_success("local-model", "ok").unwrap();
        let report = run_doctor_with_env(
            DoctorOptions::offline(Some(repo), None),
            [(
                config::ENV_OLLAMA_BASE_URL.to_string(),
                server.base_url().to_string(),
            )],
        );

        assert!(report.passed, "{report:#?}");
        assert_eq!(report.mode, DoctorMode::Offline);
        assert!(server.requests().is_empty());
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.id == "providers.network"
                    && check.status == DiagnosticStatus::Skipped)
        );
    }

    #[test]
    fn offline_doctor_reports_invalid_repo_as_readiness_failure() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("not-git");
        fs::create_dir_all(&repo).unwrap();

        let report = run_doctor_with_env(
            DoctorOptions::offline(Some(repo), None),
            std::iter::empty::<(String, String)>(),
        );

        assert!(!report.passed);
        assert_eq!(report.summary.failed, 1);
        assert_eq!(report.checks[0].id, "repo.discovery");
    }

    #[test]
    fn provider_local_doctor_checks_fake_ollama_models() {
        let temp = tempfile::tempdir().unwrap();
        let repo = init_repo_with_config(temp.path().join("repo"), |config| {
            config.providers.ollama.base_url = FakeHttpServer::ollama_success("local-model", "ok")
                .unwrap()
                .base_url();
        });
        let server = FakeHttpServer::ollama_success("local-model", "ok").unwrap();
        write_repo_config(&repo, |config| {
            config.providers.ollama.base_url = server.base_url();
            config.providers.ollama.default_model = "local-model".to_string();
        });

        let report = run_doctor_with_env(
            DoctorOptions {
                repo: Some(repo),
                state_dir: None,
                mode: DoctorMode::ProvidersLocal,
                deep: false,
            },
            std::iter::empty::<(String, String)>(),
        );

        assert!(report.passed, "{report:#?}");
        assert_eq!(server.requests().len(), 1);
        assert_eq!(server.requests()[0].path, "/api/tags");
    }

    #[test]
    fn provider_all_doctor_checks_fake_ollama_and_openai_models() {
        let temp = tempfile::tempdir().unwrap();
        let repo = init_test_repo(temp.path().join("repo"));
        config::init_repo(Some(&repo)).unwrap();
        let ollama = FakeHttpServer::ollama_success("local-model", "ok").unwrap();
        let openai = FakeHttpServer::openai_success("gpt-test", "resp_1", "ok").unwrap();
        write_repo_config(&repo, |config| {
            config.providers.ollama.base_url = ollama.base_url();
            config.providers.ollama.default_model = "local-model".to_string();
            config.providers.openai.base_url = openai.base_url();
            config.providers.openai.default_model = "gpt-test".to_string();
            config.providers.openai.allow_untrusted_provider_url = true;
        });

        let report = run_doctor_with_env(
            DoctorOptions {
                repo: Some(repo),
                state_dir: None,
                mode: DoctorMode::ProvidersAll,
                deep: false,
            },
            [("ARM_OPENAI_API_KEY".to_string(), "test-key".to_string())],
        );

        assert!(report.passed, "{report:#?}");
        assert!(
            ollama
                .requests()
                .iter()
                .any(|request| request.path == "/api/tags")
        );
        assert!(
            openai
                .requests()
                .iter()
                .any(|request| request.path == "/models")
        );
    }

    #[test]
    fn deep_doctor_runs_tiny_generation_against_fakes() {
        let temp = tempfile::tempdir().unwrap();
        let repo = init_test_repo(temp.path().join("repo"));
        config::init_repo(Some(&repo)).unwrap();
        let ollama = FakeHttpServer::ollama_success("local-model", "ok").unwrap();
        let openai = FakeHttpServer::openai_success("gpt-test", "resp_1", "ok").unwrap();
        write_repo_config(&repo, |config| {
            config.providers.ollama.base_url = ollama.base_url();
            config.providers.ollama.default_model = "local-model".to_string();
            config.providers.openai.base_url = openai.base_url();
            config.providers.openai.default_model = "gpt-test".to_string();
            config.providers.openai.allow_untrusted_provider_url = true;
        });

        let report = run_doctor_with_env(
            DoctorOptions {
                repo: Some(repo),
                state_dir: None,
                mode: DoctorMode::ProvidersAll,
                deep: true,
            },
            [("ARM_OPENAI_API_KEY".to_string(), "test-key".to_string())],
        );

        assert!(report.passed, "{report:#?}");
        let ollama_generate = ollama
            .requests()
            .into_iter()
            .find(|request| request.path == "/api/generate")
            .unwrap()
            .json_body()
            .unwrap();
        assert_eq!(ollama_generate["options"]["num_predict"], 16);
        let openai_response = openai
            .requests()
            .into_iter()
            .find(|request| request.path == "/responses")
            .unwrap()
            .json_body()
            .unwrap();
        assert_eq!(openai_response["max_output_tokens"], 16);
    }

    #[test]
    fn provider_errors_are_redacted_in_report() {
        let temp = tempfile::tempdir().unwrap();
        let repo = init_test_repo(temp.path().join("repo"));
        config::init_repo(Some(&repo)).unwrap();
        let secret = "sk-testsecret12345678901234567890";
        let server = FakeHttpServer::start(vec![FakeHttpRoute::new(
            "GET",
            "/api/tags",
            FakeHttpResponse::json(
                500,
                serde_json::json!({ "error": format!("token={secret}") }),
            ),
        )])
        .unwrap();
        write_repo_config(&repo, |config| {
            config.providers.ollama.base_url = server.base_url();
            config.providers.ollama.default_model = "local-model".to_string();
        });

        let report = run_doctor_with_env(
            DoctorOptions {
                repo: Some(repo),
                state_dir: None,
                mode: DoctorMode::ProvidersLocal,
                deep: false,
            },
            std::iter::empty::<(String, String)>(),
        );

        assert!(!report.passed);
        let encoded = serde_json::to_string(&report).unwrap();
        assert!(!encoded.contains(secret), "{encoded}");
        assert!(encoded.contains("[REDACTED"));
    }

    fn init_repo_with_config(repo: PathBuf, configure: impl FnOnce(&mut HarnessConfig)) -> PathBuf {
        let repo = init_test_repo(repo);
        config::init_repo(Some(&repo)).unwrap();
        write_repo_config(&repo, configure);
        repo
    }

    fn write_repo_config(repo: &Path, configure: impl FnOnce(&mut HarnessConfig)) {
        let mut harness_config = HarnessConfig::default();
        configure(&mut harness_config);
        config::write_config(repo, &harness_config).unwrap();
    }

    fn init_test_repo(repo: PathBuf) -> PathBuf {
        fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init"]);
        run_git(&repo, &["config", "user.email", "doctor@example.invalid"]);
        run_git(&repo, &["config", "user.name", "Doctor Test"]);
        fs::write(repo.join("README.md"), "# doctor test\n").unwrap();
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "initial"]);
        repo
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
