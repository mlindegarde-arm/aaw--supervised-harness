pub use crate::domain::HarnessConfig;

use crate::domain::{
    CommandConfig, OllamaConfig, OpenAiConfig, OrchestratorConfig, ProvidersConfig, WorkspaceConfig,
};
use crate::error::{HarnessError, HarnessResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

pub const HARNESS_DIR_NAME: &str = ".harness";
pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const STATE_FILE_NAME: &str = "state.sqlite";
pub const LOGS_DIR_NAME: &str = "logs";
pub const ARTIFACTS_DIR_NAME: &str = "artifacts";
pub const DEFAULT_WORKTREE_DIR_NAME: &str = ".harness-worktrees";

pub const ENV_OLLAMA_BASE_URL: &str = "HARNESS_OLLAMA_BASE_URL";
pub const ENV_OPENAI_BASE_URL: &str = "HARNESS_OPENAI_BASE_URL";
pub const ENV_ALLOW_UNTRUSTED_PROVIDER_URL: &str = "HARNESS_ALLOW_UNTRUSTED_PROVIDER_URL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub repo_root: PathBuf,
    pub state_dir: PathBuf,
    pub config_file: PathBuf,
    pub state_file: PathBuf,
    pub logs_dir: PathBuf,
    pub artifacts_dir: PathBuf,
    pub worktree_root: PathBuf,
}

impl ConfigPaths {
    pub fn for_repo(repo_root: impl Into<PathBuf>, config: &HarnessConfig) -> HarnessResult<Self> {
        let repo_root = normalize_existing_path(repo_root.into())?;
        let state_dir = normalize_configured_path(&repo_root, &config.workspace.state_dir)?;
        let worktree_root = repo_worktree_root(&repo_root, &config.workspace.worktree_root)?;

        Ok(Self {
            config_file: repo_root.join(HARNESS_DIR_NAME).join(CONFIG_FILE_NAME),
            state_file: state_dir.join(STATE_FILE_NAME),
            logs_dir: state_dir.join(LOGS_DIR_NAME),
            artifacts_dir: state_dir.join(ARTIFACTS_DIR_NAME),
            repo_root,
            state_dir,
            worktree_root,
        })
    }

    pub fn run_artifact_dir(&self, task_id: &str, run_id: &str) -> PathBuf {
        self.artifacts_dir.join(task_id).join(run_id)
    }

    pub fn run_log_dir(&self, task_id: &str, run_id: &str) -> PathBuf {
        self.logs_dir.join(task_id).join(run_id)
    }

    pub fn validation_log_path(&self, task_id: &str, run_id: &str, attempt_number: u32) -> PathBuf {
        self.run_log_dir(task_id, run_id)
            .join(format!("attempt_{attempt_number:03}.validation.log"))
    }

    pub fn task_worktree_path(&self, task_id: &str) -> PathBuf {
        self.worktree_root.join(task_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    pub config: HarnessConfig,
    pub paths: ConfigPaths,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitResult {
    pub paths: ConfigPaths,
    pub config_created: bool,
    pub harness_gitignore_warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionIssue {
    pub path: PathBuf,
    pub expected_max_mode: u32,
    pub actual_mode: u32,
}

pub fn default_config() -> HarnessConfig {
    HarnessConfig::default()
}

pub fn discover_repo_root(repo: Option<&Path>) -> HarnessResult<PathBuf> {
    match repo {
        Some(path) => {
            let root = normalize_existing_path(path.to_path_buf())?;
            ensure_git_repository(&root)?;
            Ok(root)
        }
        None => {
            let output = Command::new("git")
                .args(["rev-parse", "--show-toplevel"])
                .output()
                .map_err(|err| {
                    HarnessError::External(format!("failed to run git rev-parse: {err}"))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(HarnessError::Usage(if stderr.is_empty() {
                    "not inside a git repository".to_string()
                } else {
                    format!("not inside a git repository: {stderr}")
                }));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            normalize_existing_path(PathBuf::from(stdout.trim()))
        }
    }
}

pub fn load_config(repo: Option<&Path>) -> HarnessResult<LoadedConfig> {
    load_config_with_env(repo, std::env::vars())
}

pub fn load_config_with_env<I, K, V>(repo: Option<&Path>, env: I) -> HarnessResult<LoadedConfig>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let repo_root = discover_repo_root(repo)?;
    let config_path = repo_root.join(HARNESS_DIR_NAME).join(CONFIG_FILE_NAME);
    let mut config = if config_path.exists() {
        parse_config_toml(&fs::read_to_string(&config_path).map_err(|err| {
            HarnessError::InvalidConfig(format!(
                "failed to read config {}: {err}",
                config_path.display()
            ))
        })?)?
    } else {
        default_config()
    };

    apply_env_overrides(&mut config, env)?;
    normalize_loaded_paths(&repo_root, &mut config)?;
    let paths = ConfigPaths::for_repo(&repo_root, &config)?;
    Ok(LoadedConfig { config, paths })
}

pub fn init_repo(repo: Option<&Path>) -> HarnessResult<InitResult> {
    let repo_root = discover_repo_root(repo)?;
    let mut config = default_config();
    normalize_loaded_paths(&repo_root, &mut config)?;
    let paths = ConfigPaths::for_repo(&repo_root, &config)?;

    ensure_private_dir(&paths.state_dir)?;
    ensure_private_dir(&paths.logs_dir)?;
    ensure_private_dir(&paths.artifacts_dir)?;

    let config_created = if paths.config_file.exists() {
        set_private_file_permissions(&paths.config_file)?;
        false
    } else {
        write_private_file(
            &paths.config_file,
            format_config_toml(&default_config()).as_bytes(),
        )?;
        true
    };

    let harness_gitignore_warning = if harness_is_ignored(&repo_root)? {
        None
    } else {
        Some(format!(
            "{HARNESS_DIR_NAME}/ is not ignored by git; add it to .gitignore"
        ))
    };

    Ok(InitResult {
        paths,
        config_created,
        harness_gitignore_warning,
    })
}

pub fn write_config(repo_root: &Path, config: &HarnessConfig) -> HarnessResult<PathBuf> {
    let repo_root = normalize_existing_path(repo_root.to_path_buf())?;
    let state_dir = repo_root.join(HARNESS_DIR_NAME);
    ensure_private_dir(&state_dir)?;
    let config_file = state_dir.join(CONFIG_FILE_NAME);
    write_private_file(&config_file, format_config_toml(config).as_bytes())?;
    Ok(config_file)
}

pub fn parse_config_toml(input: &str) -> HarnessResult<HarnessConfig> {
    let wire: WireConfig = toml::from_str(input)
        .map_err(|err| HarnessError::InvalidConfig(format!("failed to parse TOML: {err}")))?;
    Ok(wire.into())
}

pub fn format_config_toml(config: &HarnessConfig) -> String {
    format!(
        r#"[workspace]
state_dir = "{state_dir}"
worktree_root = "{worktree_root}"

[command]
shell_path = "{shell_path}"
non_interactive_stdin = {non_interactive_stdin}
kill_process_group_on_timeout = {kill_process_group_on_timeout}

[orchestrator]
max_attempts = {max_attempts}
max_invalid_responses = {max_invalid_responses}
max_provider_failures = {max_provider_failures}
max_escalation_cycles = {max_escalation_cycles}
validation_timeout_seconds = {validation_timeout_seconds}
max_validation_output_bytes = {max_validation_output_bytes}
max_patch_bytes = {max_patch_bytes}
max_files_changed = {max_files_changed}
max_total_runtime_seconds = {max_total_runtime_seconds}

[providers.ollama]
base_url = "{ollama_base_url}"
default_model = "{ollama_default_model}"
connect_timeout_seconds = {ollama_connect_timeout_seconds}
timeout_seconds = {ollama_timeout_seconds}
max_retries = {ollama_max_retries}
retry_backoff_ms = {ollama_retry_backoff_ms}
num_ctx = {ollama_num_ctx}
num_predict = {ollama_num_predict}
temperature = {ollama_temperature:.1}
seed = {ollama_seed}
keep_alive = "{ollama_keep_alive}"

[providers.openai]
base_url = "{openai_base_url}"
api_key_env = "{openai_api_key_env}"
fallback_api_key_env = "{openai_fallback_api_key_env}"
default_model = "{openai_default_model}"
connect_timeout_seconds = {openai_connect_timeout_seconds}
timeout_seconds = {openai_timeout_seconds}
max_retries = {openai_max_retries}
retry_backoff_ms = {openai_retry_backoff_ms}
max_output_tokens = {openai_max_output_tokens}
allow_untrusted_provider_url = {openai_allow_untrusted_provider_url}
"#,
        state_dir = escape_toml_string(&config.workspace.state_dir),
        worktree_root = escape_toml_string(&config.workspace.worktree_root),
        shell_path = escape_toml_string(&config.command.shell_path),
        non_interactive_stdin = config.command.non_interactive_stdin,
        kill_process_group_on_timeout = config.command.kill_process_group_on_timeout,
        max_attempts = config.orchestrator.max_attempts,
        max_invalid_responses = config.orchestrator.max_invalid_responses,
        max_provider_failures = config.orchestrator.max_provider_failures,
        max_escalation_cycles = config.orchestrator.max_escalation_cycles,
        validation_timeout_seconds = config.orchestrator.validation_timeout_seconds,
        max_validation_output_bytes = config.orchestrator.max_validation_output_bytes,
        max_patch_bytes = config.orchestrator.max_patch_bytes,
        max_files_changed = config.orchestrator.max_files_changed,
        max_total_runtime_seconds = config.orchestrator.max_total_runtime_seconds,
        ollama_base_url = escape_toml_string(&config.providers.ollama.base_url),
        ollama_default_model = escape_toml_string(&config.providers.ollama.default_model),
        ollama_connect_timeout_seconds = config.providers.ollama.connect_timeout_seconds,
        ollama_timeout_seconds = config.providers.ollama.timeout_seconds,
        ollama_max_retries = config.providers.ollama.max_retries,
        ollama_retry_backoff_ms = config.providers.ollama.retry_backoff_ms,
        ollama_num_ctx = config.providers.ollama.num_ctx,
        ollama_num_predict = config.providers.ollama.num_predict,
        ollama_temperature = config.providers.ollama.temperature,
        ollama_seed = config.providers.ollama.seed,
        ollama_keep_alive = escape_toml_string(&config.providers.ollama.keep_alive),
        openai_base_url = escape_toml_string(&config.providers.openai.base_url),
        openai_api_key_env = escape_toml_string(&config.providers.openai.api_key_env),
        openai_fallback_api_key_env =
            escape_toml_string(&config.providers.openai.fallback_api_key_env),
        openai_default_model = escape_toml_string(&config.providers.openai.default_model),
        openai_connect_timeout_seconds = config.providers.openai.connect_timeout_seconds,
        openai_timeout_seconds = config.providers.openai.timeout_seconds,
        openai_max_retries = config.providers.openai.max_retries,
        openai_retry_backoff_ms = config.providers.openai.retry_backoff_ms,
        openai_max_output_tokens = config.providers.openai.max_output_tokens,
        openai_allow_untrusted_provider_url = config.providers.openai.allow_untrusted_provider_url,
    )
}

pub fn normalize_configured_path(repo_root: &Path, configured: &str) -> HarnessResult<PathBuf> {
    if configured.trim().is_empty() {
        return Err(HarnessError::InvalidConfig(
            "configured path must not be empty".to_string(),
        ));
    }

    let path = PathBuf::from(configured);
    let absolute = if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    };

    normalize_absolute_path(&absolute)
}

pub fn normalize_worktree_root(repo_root: &Path, configured: &str) -> HarnessResult<PathBuf> {
    let root = normalize_configured_path(repo_root, configured)?;
    ensure_worktree_root_outside_repo(repo_root, &root)?;
    Ok(root)
}

pub fn repo_worktree_root(repo_root: &Path, configured: &str) -> HarnessResult<PathBuf> {
    let base = normalize_worktree_root(repo_root, configured)?;
    let root = base.join(repo_name_hash(repo_root)?);
    ensure_worktree_root_outside_repo(repo_root, &root)?;
    Ok(root)
}

pub fn ensure_worktree_root_outside_repo(
    repo_root: &Path,
    worktree_root: &Path,
) -> HarnessResult<()> {
    let repo_root = normalize_absolute_path(repo_root)?;
    let worktree_root = normalize_absolute_path(worktree_root)?;
    let repo_root_resolved = resolve_existing_prefix(&repo_root)?;
    let worktree_root_resolved = resolve_existing_prefix(&worktree_root)?;

    if worktree_root == repo_root
        || worktree_root.starts_with(&repo_root)
        || worktree_root_resolved == repo_root_resolved
        || worktree_root_resolved.starts_with(&repo_root_resolved)
    {
        return Err(HarnessError::InvalidConfig(format!(
            "worktree root {} must be outside repository root {}",
            worktree_root.display(),
            repo_root.display()
        )));
    }

    Ok(())
}

pub fn ensure_private_dir(path: &Path) -> HarnessResult<()> {
    fs::create_dir_all(path).map_err(|err| {
        HarnessError::External(format!(
            "failed to create directory {}: {err}",
            path.display()
        ))
    })?;
    set_private_dir_permissions(path)
}

pub fn write_private_file(path: &Path, bytes: &[u8]) -> HarnessResult<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }

    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    set_open_options_private_file_mode(&mut options);
    let mut file = options.open(path).map_err(|err| {
        HarnessError::External(format!("failed to open file {}: {err}", path.display()))
    })?;
    file.write_all(bytes).map_err(|err| {
        HarnessError::External(format!("failed to write file {}: {err}", path.display()))
    })?;
    file.sync_all().map_err(|err| {
        HarnessError::External(format!("failed to sync file {}: {err}", path.display()))
    })?;
    set_private_file_permissions(path)
}

pub fn private_permission_issues(paths: &[PathBuf]) -> HarnessResult<Vec<PermissionIssue>> {
    let mut issues = Vec::new();
    for path in paths {
        if path.exists() {
            collect_permission_issue(path, &mut issues)?;
        }
    }
    Ok(issues)
}

fn normalize_loaded_paths(repo_root: &Path, config: &mut HarnessConfig) -> HarnessResult<()> {
    let state_dir = normalize_configured_path(repo_root, &config.workspace.state_dir)?;
    let worktree_root = normalize_worktree_root(repo_root, &config.workspace.worktree_root)?;
    config.workspace.state_dir = path_to_string(state_dir)?;
    config.workspace.worktree_root = path_to_string(worktree_root)?;
    Ok(())
}

fn apply_env_overrides<I, K, V>(config: &mut HarnessConfig, env: I) -> HarnessResult<()>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let env: BTreeMap<String, String> = env
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();

    if let Some(value) = env.get(ENV_OLLAMA_BASE_URL) {
        config.providers.ollama.base_url = trim_trailing_slashes(value);
    }
    if let Some(value) = env.get(ENV_OPENAI_BASE_URL) {
        config.providers.openai.base_url = trim_trailing_slashes(value);
    }
    if let Some(value) = env.get(ENV_ALLOW_UNTRUSTED_PROVIDER_URL) {
        config.providers.openai.allow_untrusted_provider_url =
            parse_env_bool(ENV_ALLOW_UNTRUSTED_PROVIDER_URL, value)?;
    }

    Ok(())
}

fn parse_env_bool(key: &str, value: &str) -> HarnessResult<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(HarnessError::InvalidConfig(format!(
            "{key} must be a boolean value"
        ))),
    }
}

fn ensure_git_repository(repo_root: &Path) -> HarnessResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|err| HarnessError::External(format!("failed to run git rev-parse: {err}")))?;

    if !output.status.success() {
        return Err(HarnessError::Usage(format!(
            "{} is not a git repository",
            repo_root.display()
        )));
    }

    let actual = normalize_existing_path(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))?;
    if actual != repo_root {
        return Err(HarnessError::Usage(format!(
            "{} is inside git repository {}; pass the repository root",
            repo_root.display(),
            actual.display()
        )));
    }

    Ok(())
}

fn harness_is_ignored(repo_root: &Path) -> HarnessResult<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["check-ignore", "-q", HARNESS_DIR_NAME])
        .status()
        .map_err(|err| HarnessError::External(format!("failed to run git check-ignore: {err}")))?;

    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(HarnessError::External(format!(
            "git check-ignore failed for {HARNESS_DIR_NAME}"
        ))),
    }
}

fn normalize_existing_path(path: PathBuf) -> HarnessResult<PathBuf> {
    path.canonicalize().map_err(|err| {
        HarnessError::External(format!("failed to canonicalize {}: {err}", path.display()))
    })
}

fn normalize_absolute_path(path: &Path) -> HarnessResult<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(HarnessError::InvalidConfig(format!(
                        "path {} escapes filesystem root",
                        path.display()
                    )));
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    if !normalized.is_absolute() {
        return Err(HarnessError::InvalidConfig(format!(
            "path {} did not normalize to an absolute path",
            path.display()
        )));
    }

    Ok(normalized)
}

fn resolve_existing_prefix(path: &Path) -> HarnessResult<PathBuf> {
    let mut existing = path;
    let mut missing = Vec::new();

    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            break;
        };
        missing.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            HarnessError::InvalidConfig(format!("path {} has no existing parent", path.display()))
        })?;
    }

    let mut resolved = fs::canonicalize(existing).map_err(|err| {
        HarnessError::InvalidConfig(format!(
            "failed to resolve path prefix {}: {err}",
            existing.display()
        ))
    })?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }

    normalize_absolute_path(&resolved)
}

fn repo_name_hash(repo_root: &Path) -> HarnessResult<String> {
    let name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo");
    let safe_name: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    Ok(format!(
        "{}-{:016x}",
        safe_name,
        stable_hash(path_to_string(repo_root)?)
    ))
}

fn stable_hash(value: impl Hash) -> u64 {
    let mut hasher = Fnv1a64::default();
    value.hash(&mut hasher);
    hasher.finish()
}

#[derive(Default)]
struct Fnv1a64(u64);

impl Hasher for Fnv1a64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut hash = if self.0 == 0 {
            0xcbf29ce484222325
        } else {
            self.0
        };
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        self.0 = hash;
    }
}

fn trim_trailing_slashes(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

fn path_to_string(path: impl AsRef<Path>) -> HarnessResult<String> {
    path.as_ref()
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            HarnessError::InvalidConfig(format!("non-UTF-8 path {}", path.as_ref().display()))
        })
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(unix)]
fn set_open_options_private_file_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_open_options_private_file_mode(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> HarnessResult<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|err| {
        HarnessError::External(format!(
            "failed to set directory permissions on {}: {err}",
            path.display()
        ))
    })
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> HarnessResult<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> HarnessResult<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
        HarnessError::External(format!(
            "failed to set file permissions on {}: {err}",
            path.display()
        ))
    })
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> HarnessResult<()> {
    Ok(())
}

#[cfg(unix)]
fn collect_permission_issue(path: &Path, issues: &mut Vec<PermissionIssue>) -> HarnessResult<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::metadata(path).map_err(|err| {
        HarnessError::External(format!(
            "failed to read metadata for {}: {err}",
            path.display()
        ))
    })?;
    let expected_max_mode = if metadata.is_dir() { 0o700 } else { 0o600 };
    let actual_mode = metadata.permissions().mode() & 0o777;
    if actual_mode & !expected_max_mode != 0 {
        issues.push(PermissionIssue {
            path: path.to_path_buf(),
            expected_max_mode,
            actual_mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn collect_permission_issue(_path: &Path, _issues: &mut Vec<PermissionIssue>) -> HarnessResult<()> {
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct WireConfig {
    workspace: WireWorkspaceConfig,
    command: WireCommandConfig,
    orchestrator: WireOrchestratorConfig,
    providers: WireProvidersConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct WireWorkspaceConfig {
    state_dir: String,
    worktree_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct WireCommandConfig {
    shell_path: String,
    non_interactive_stdin: bool,
    kill_process_group_on_timeout: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct WireOrchestratorConfig {
    max_attempts: u32,
    max_invalid_responses: u32,
    max_provider_failures: u32,
    max_escalation_cycles: u32,
    validation_timeout_seconds: u64,
    max_validation_output_bytes: u64,
    max_patch_bytes: u64,
    max_files_changed: u32,
    max_total_runtime_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct WireProvidersConfig {
    ollama: WireOllamaConfig,
    openai: WireOpenAiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct WireOllamaConfig {
    base_url: String,
    default_model: String,
    connect_timeout_seconds: u64,
    timeout_seconds: u64,
    max_retries: u32,
    retry_backoff_ms: u64,
    num_ctx: u32,
    num_predict: u32,
    temperature: f32,
    seed: u32,
    keep_alive: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct WireOpenAiConfig {
    base_url: String,
    api_key_env: String,
    fallback_api_key_env: String,
    default_model: String,
    connect_timeout_seconds: u64,
    timeout_seconds: u64,
    max_retries: u32,
    retry_backoff_ms: u64,
    max_output_tokens: u32,
    allow_untrusted_provider_url: bool,
}

impl Default for WireConfig {
    fn default() -> Self {
        HarnessConfig::default().into()
    }
}

impl Default for WireWorkspaceConfig {
    fn default() -> Self {
        HarnessConfig::default().workspace.into()
    }
}

impl Default for WireCommandConfig {
    fn default() -> Self {
        HarnessConfig::default().command.into()
    }
}

impl Default for WireOrchestratorConfig {
    fn default() -> Self {
        HarnessConfig::default().orchestrator.into()
    }
}

impl Default for WireProvidersConfig {
    fn default() -> Self {
        HarnessConfig::default().providers.into()
    }
}

impl Default for WireOllamaConfig {
    fn default() -> Self {
        HarnessConfig::default().providers.ollama.into()
    }
}

impl Default for WireOpenAiConfig {
    fn default() -> Self {
        HarnessConfig::default().providers.openai.into()
    }
}

impl From<HarnessConfig> for WireConfig {
    fn from(config: HarnessConfig) -> Self {
        Self {
            workspace: config.workspace.into(),
            command: config.command.into(),
            orchestrator: config.orchestrator.into(),
            providers: config.providers.into(),
        }
    }
}

impl From<WorkspaceConfig> for WireWorkspaceConfig {
    fn from(config: WorkspaceConfig) -> Self {
        Self {
            state_dir: config.state_dir,
            worktree_root: config.worktree_root,
        }
    }
}

impl From<CommandConfig> for WireCommandConfig {
    fn from(config: CommandConfig) -> Self {
        Self {
            shell_path: config.shell_path,
            non_interactive_stdin: config.non_interactive_stdin,
            kill_process_group_on_timeout: config.kill_process_group_on_timeout,
        }
    }
}

impl From<OrchestratorConfig> for WireOrchestratorConfig {
    fn from(config: OrchestratorConfig) -> Self {
        Self {
            max_attempts: config.max_attempts,
            max_invalid_responses: config.max_invalid_responses,
            max_provider_failures: config.max_provider_failures,
            max_escalation_cycles: config.max_escalation_cycles,
            validation_timeout_seconds: config.validation_timeout_seconds,
            max_validation_output_bytes: config.max_validation_output_bytes,
            max_patch_bytes: config.max_patch_bytes,
            max_files_changed: config.max_files_changed,
            max_total_runtime_seconds: config.max_total_runtime_seconds,
        }
    }
}

impl From<ProvidersConfig> for WireProvidersConfig {
    fn from(config: ProvidersConfig) -> Self {
        Self {
            ollama: config.ollama.into(),
            openai: config.openai.into(),
        }
    }
}

impl From<OllamaConfig> for WireOllamaConfig {
    fn from(config: OllamaConfig) -> Self {
        Self {
            base_url: config.base_url,
            default_model: config.default_model,
            connect_timeout_seconds: config.connect_timeout_seconds,
            timeout_seconds: config.timeout_seconds,
            max_retries: config.max_retries,
            retry_backoff_ms: config.retry_backoff_ms,
            num_ctx: config.num_ctx,
            num_predict: config.num_predict,
            temperature: config.temperature,
            seed: config.seed,
            keep_alive: config.keep_alive,
        }
    }
}

impl From<OpenAiConfig> for WireOpenAiConfig {
    fn from(config: OpenAiConfig) -> Self {
        Self {
            base_url: config.base_url,
            api_key_env: config.api_key_env,
            fallback_api_key_env: config.fallback_api_key_env,
            default_model: config.default_model,
            connect_timeout_seconds: config.connect_timeout_seconds,
            timeout_seconds: config.timeout_seconds,
            max_retries: config.max_retries,
            retry_backoff_ms: config.retry_backoff_ms,
            max_output_tokens: config.max_output_tokens,
            allow_untrusted_provider_url: config.allow_untrusted_provider_url,
        }
    }
}

impl From<WireConfig> for HarnessConfig {
    fn from(config: WireConfig) -> Self {
        Self {
            workspace: config.workspace.into(),
            command: config.command.into(),
            orchestrator: config.orchestrator.into(),
            providers: config.providers.into(),
        }
    }
}

impl From<WireWorkspaceConfig> for WorkspaceConfig {
    fn from(config: WireWorkspaceConfig) -> Self {
        Self {
            state_dir: config.state_dir,
            worktree_root: config.worktree_root,
        }
    }
}

impl From<WireCommandConfig> for CommandConfig {
    fn from(config: WireCommandConfig) -> Self {
        Self {
            shell_path: config.shell_path,
            non_interactive_stdin: config.non_interactive_stdin,
            kill_process_group_on_timeout: config.kill_process_group_on_timeout,
        }
    }
}

impl From<WireOrchestratorConfig> for OrchestratorConfig {
    fn from(config: WireOrchestratorConfig) -> Self {
        Self {
            max_attempts: config.max_attempts,
            max_invalid_responses: config.max_invalid_responses,
            max_provider_failures: config.max_provider_failures,
            max_escalation_cycles: config.max_escalation_cycles,
            validation_timeout_seconds: config.validation_timeout_seconds,
            max_validation_output_bytes: config.max_validation_output_bytes,
            max_patch_bytes: config.max_patch_bytes,
            max_files_changed: config.max_files_changed,
            max_total_runtime_seconds: config.max_total_runtime_seconds,
        }
    }
}

impl From<WireProvidersConfig> for ProvidersConfig {
    fn from(config: WireProvidersConfig) -> Self {
        Self {
            ollama: config.ollama.into(),
            openai: config.openai.into(),
        }
    }
}

impl From<WireOllamaConfig> for OllamaConfig {
    fn from(config: WireOllamaConfig) -> Self {
        Self {
            base_url: trim_trailing_slashes(&config.base_url),
            default_model: config.default_model,
            connect_timeout_seconds: config.connect_timeout_seconds,
            timeout_seconds: config.timeout_seconds,
            max_retries: config.max_retries,
            retry_backoff_ms: config.retry_backoff_ms,
            num_ctx: config.num_ctx,
            num_predict: config.num_predict,
            temperature: config.temperature,
            seed: config.seed,
            keep_alive: config.keep_alive,
        }
    }
}

impl From<WireOpenAiConfig> for OpenAiConfig {
    fn from(config: WireOpenAiConfig) -> Self {
        Self {
            base_url: trim_trailing_slashes(&config.base_url),
            api_key_env: config.api_key_env,
            fallback_api_key_env: config.fallback_api_key_env,
            default_model: config.default_model,
            connect_timeout_seconds: config.connect_timeout_seconds,
            timeout_seconds: config.timeout_seconds,
            max_retries: config.max_retries,
            retry_backoff_ms: config.retry_backoff_ms,
            max_output_tokens: config.max_output_tokens,
            allow_untrusted_provider_url: config.allow_untrusted_provider_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn default_config_values_match_design() {
        let config = default_config();

        assert_eq!(config.workspace.state_dir, ".harness");
        assert_eq!(config.workspace.worktree_root, "../.harness-worktrees");
        assert_eq!(config.command.shell_path, "/bin/sh");
        assert!(config.command.non_interactive_stdin);
        assert!(config.command.kill_process_group_on_timeout);
        assert_eq!(config.orchestrator.max_attempts, 3);
        assert_eq!(config.orchestrator.max_invalid_responses, 2);
        assert_eq!(config.orchestrator.max_provider_failures, 2);
        assert_eq!(config.orchestrator.max_escalation_cycles, 1);
        assert_eq!(config.orchestrator.validation_timeout_seconds, 120);
        assert_eq!(config.orchestrator.max_validation_output_bytes, 65_536);
        assert_eq!(config.orchestrator.max_patch_bytes, 131_072);
        assert_eq!(config.orchestrator.max_files_changed, 20);
        assert_eq!(config.orchestrator.max_total_runtime_seconds, 900);
        assert_eq!(config.providers.ollama.base_url, "http://localhost:11434");
        assert_eq!(
            config.providers.ollama.default_model,
            "maternion/strand-rust-coder:latest"
        );
        assert_eq!(config.providers.ollama.connect_timeout_seconds, 10);
        assert_eq!(config.providers.ollama.timeout_seconds, 120);
        assert_eq!(config.providers.ollama.max_retries, 1);
        assert_eq!(config.providers.ollama.retry_backoff_ms, 500);
        assert_eq!(config.providers.ollama.num_ctx, 8192);
        assert_eq!(config.providers.ollama.num_predict, 2048);
        assert_eq!(config.providers.ollama.temperature, 0.0);
        assert_eq!(config.providers.ollama.seed, 42);
        assert_eq!(config.providers.ollama.keep_alive, "5m");
        assert_eq!(
            config.providers.openai.base_url,
            "https://openai-api-proxy.geo.arm.com/api/providers/openai-us/v1"
        );
        assert_eq!(config.providers.openai.api_key_env, "OPENAI_API_KEY");
        assert_eq!(
            config.providers.openai.fallback_api_key_env,
            "ARM_OPENAI_API_KEY"
        );
        assert_eq!(config.providers.openai.default_model, "gpt-5.3-codex");
        assert_eq!(config.providers.openai.connect_timeout_seconds, 10);
        assert_eq!(config.providers.openai.timeout_seconds, 120);
        assert_eq!(config.providers.openai.max_retries, 1);
        assert_eq!(config.providers.openai.retry_backoff_ms, 500);
        assert_eq!(config.providers.openai.max_output_tokens, 4096);
        assert!(!config.providers.openai.allow_untrusted_provider_url);
    }

    #[test]
    fn default_config_toml_shape_matches_design() {
        let toml = format_config_toml(&default_config());

        assert_eq!(
            toml,
            r#"[workspace]
state_dir = ".harness"
worktree_root = "../.harness-worktrees"

[command]
shell_path = "/bin/sh"
non_interactive_stdin = true
kill_process_group_on_timeout = true

[orchestrator]
max_attempts = 3
max_invalid_responses = 2
max_provider_failures = 2
max_escalation_cycles = 1
validation_timeout_seconds = 120
max_validation_output_bytes = 65536
max_patch_bytes = 131072
max_files_changed = 20
max_total_runtime_seconds = 900

[providers.ollama]
base_url = "http://localhost:11434"
default_model = "maternion/strand-rust-coder:latest"
connect_timeout_seconds = 10
timeout_seconds = 120
max_retries = 1
retry_backoff_ms = 500
num_ctx = 8192
num_predict = 2048
temperature = 0.0
seed = 42
keep_alive = "5m"

[providers.openai]
base_url = "https://openai-api-proxy.geo.arm.com/api/providers/openai-us/v1"
api_key_env = "OPENAI_API_KEY"
fallback_api_key_env = "ARM_OPENAI_API_KEY"
default_model = "gpt-5.3-codex"
connect_timeout_seconds = 10
timeout_seconds = 120
max_retries = 1
retry_backoff_ms = 500
max_output_tokens = 4096
allow_untrusted_provider_url = false
"#
        );
    }

    #[test]
    fn config_toml_round_trips_without_api_keys() {
        let parsed = parse_config_toml(&format_config_toml(&default_config())).unwrap();

        assert_eq!(parsed, default_config());
        assert!(!format_config_toml(&default_config()).contains("sk-"));
    }

    #[test]
    fn env_overrides_used_by_tests_are_applied() {
        let repo = git_fixture(false);
        let loaded = load_config_with_env(
            Some(repo.path()),
            [
                (ENV_OLLAMA_BASE_URL, "http://127.0.0.1:3001/"),
                (ENV_OPENAI_BASE_URL, "http://127.0.0.1:3002/v1/"),
                (ENV_ALLOW_UNTRUSTED_PROVIDER_URL, "true"),
            ],
        )
        .unwrap();

        assert_eq!(
            loaded.config.providers.ollama.base_url,
            "http://127.0.0.1:3001"
        );
        assert_eq!(
            loaded.config.providers.openai.base_url,
            "http://127.0.0.1:3002/v1"
        );
        assert!(loaded.config.providers.openai.allow_untrusted_provider_url);
    }

    #[test]
    fn init_creates_expected_paths_and_warns_when_not_ignored() {
        let repo = git_fixture(false);

        let result = init_repo(Some(repo.path())).unwrap();

        assert!(result.config_created);
        assert!(result.paths.state_dir.is_dir());
        assert!(result.paths.logs_dir.is_dir());
        assert!(result.paths.artifacts_dir.is_dir());
        assert!(result.paths.config_file.is_file());
        assert_eq!(
            fs::read_to_string(&result.paths.config_file).unwrap(),
            format_config_toml(&default_config())
        );
        assert_eq!(
            result.paths.state_file,
            result.paths.state_dir.join(STATE_FILE_NAME)
        );
        assert!(result.harness_gitignore_warning.is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&result.paths.state_dir)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&result.paths.config_file)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn init_suppresses_warning_when_harness_is_ignored() {
        let repo = git_fixture(true);

        let result = init_repo(Some(repo.path())).unwrap();

        assert!(result.harness_gitignore_warning.is_none());
    }

    #[test]
    fn init_fails_outside_git_repository() {
        let temp = TempDir::new().unwrap();

        let err = init_repo(Some(temp.path())).unwrap_err();

        assert!(matches!(err, HarnessError::Usage(_)));
    }

    #[test]
    fn worktree_root_normalization_is_absolute_and_namespaced() {
        let repo = git_fixture(false);
        let root = repo_worktree_root(repo.path(), "../.harness-worktrees").unwrap();

        assert!(root.is_absolute());
        assert!(root.ends_with(repo_name_hash(repo.path()).unwrap()));
        assert!(!root.starts_with(repo.path()));
    }

    #[test]
    fn worktree_root_under_repo_is_rejected() {
        let repo = git_fixture(false);

        let err = normalize_worktree_root(repo.path(), ".harness/worktrees").unwrap_err();

        assert!(matches!(err, HarnessError::InvalidConfig(_)));
    }

    #[cfg(unix)]
    #[test]
    fn worktree_root_symlink_into_repo_is_rejected() {
        use std::os::unix::fs::symlink;

        let repo = git_fixture(false);
        let outside = TempDir::new().unwrap();
        let link = outside.path().join("link-to-repo");
        symlink(repo.path(), &link).unwrap();

        let err = normalize_worktree_root(repo.path(), link.to_str().unwrap()).unwrap_err();

        assert!(matches!(err, HarnessError::InvalidConfig(_)));
    }

    #[cfg(unix)]
    #[test]
    fn worktree_namespace_symlink_into_repo_is_rejected() {
        use std::os::unix::fs::symlink;

        let repo = git_fixture(false);
        let outside = TempDir::new().unwrap();
        let namespace = outside.path().join(repo_name_hash(repo.path()).unwrap());
        symlink(repo.path(), &namespace).unwrap();

        let err = repo_worktree_root(repo.path(), outside.path().to_str().unwrap()).unwrap_err();

        assert!(matches!(err, HarnessError::InvalidConfig(_)));
    }

    #[test]
    fn path_helpers_build_state_artifact_log_and_worktree_paths() {
        let repo = git_fixture(false);
        let loaded =
            load_config_with_env(Some(repo.path()), std::iter::empty::<(&str, &str)>()).unwrap();
        let repo_root = repo.path().canonicalize().unwrap();
        let expected_worktree_root =
            repo_worktree_root(&repo_root, &loaded.config.workspace.worktree_root).unwrap();

        assert_eq!(
            loaded.paths.config_file,
            repo_root.join(HARNESS_DIR_NAME).join(CONFIG_FILE_NAME)
        );
        assert_eq!(loaded.paths.worktree_root, expected_worktree_root);
        assert_eq!(
            loaded.paths.worktree_root.parent().unwrap(),
            Path::new(&loaded.config.workspace.worktree_root)
        );
        assert_eq!(
            loaded.paths.validation_log_path("task_123", "run_456", 7),
            loaded
                .paths
                .logs_dir
                .join("task_123")
                .join("run_456")
                .join("attempt_007.validation.log")
        );
        assert_eq!(
            loaded.paths.run_artifact_dir("task_123", "run_456"),
            loaded.paths.artifacts_dir.join("task_123").join("run_456")
        );
        assert_eq!(
            loaded.paths.task_worktree_path("task_123"),
            loaded.paths.worktree_root.join("task_123")
        );
    }

    #[test]
    fn config_file_path_stays_under_harness_with_custom_state_dir() {
        let repo = git_fixture(false);
        let repo_root = repo.path().canonicalize().unwrap();
        let mut config = default_config();
        config.workspace.state_dir = ".custom-harness-state".to_string();

        let paths = ConfigPaths::for_repo(&repo_root, &config).unwrap();

        assert_eq!(
            paths.config_file,
            repo_root.join(HARNESS_DIR_NAME).join(CONFIG_FILE_NAME)
        );
        assert_eq!(paths.state_dir, repo_root.join(".custom-harness-state"));
    }

    fn git_fixture(ignore_harness: bool) -> TempDir {
        let temp = TempDir::new().unwrap();
        run_git(temp.path(), &["init"]);
        if ignore_harness {
            fs::write(temp.path().join(".gitignore"), ".harness/\n").unwrap();
        }
        temp
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
