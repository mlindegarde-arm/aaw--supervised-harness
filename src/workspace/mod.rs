use crate::HarnessResult;
use crate::domain::{RunId, TaskId};
use crate::error::HarnessError;
use crate::patch::{PatchValidationConfig, validate_patch_safety};
use crate::security::{DefaultEnvironmentSanitizer, EnvironmentSanitizer};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedWorktree {
    pub path: String,
    pub branch: String,
    pub base_ref: Option<String>,
    pub base_commit: Option<String>,
    pub last_seen_head: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRequest {
    pub repo_root: String,
    pub worktree_root: String,
    pub task_id: TaskId,
    pub base_ref: Option<String>,
    pub recorded: Option<RecordedWorktree>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub timed_out: bool,
    pub truncated: bool,
    pub truncated_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub base_ref: String,
    pub base_commit: String,
    pub head: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub command: String,
    pub cwd: String,
    pub shell_path: String,
    pub env: BTreeMap<String, String>,
    pub timeout_seconds: u64,
    pub max_output_bytes: u64,
    pub stdin: CommandStdin,
    pub kill_process_group_on_timeout: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandStdin {
    Null,
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchCheck {
    pub worktree_path: String,
    pub diff: String,
    pub max_patch_bytes: u64,
    pub max_files_changed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchCheckResult {
    pub files_changed: Vec<String>,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchApplyResult {
    pub check: PatchCheckResult,
    pub stderr: String,
}

pub trait WorkspaceManager {
    fn discover_repo_root(&self, repo: Option<&str>) -> HarnessResult<String>;
    fn source_is_dirty(&self, repo_root: &str) -> HarnessResult<bool>;
    fn resolve_base_commit(&self, repo_root: &str, base_ref: Option<&str>)
    -> HarnessResult<String>;
    fn ensure_task_worktree(&self, request: WorktreeRequest) -> HarnessResult<WorktreeInfo>;
    fn verify_recorded_worktree(
        &self,
        repo_root: &str,
        recorded: &RecordedWorktree,
    ) -> HarnessResult<WorktreeInfo>;
    fn capture_diff(&self, worktree_path: &str, run_id: &RunId) -> HarnessResult<String>;
    fn check_patch(&self, patch: PatchCheck) -> HarnessResult<PatchCheckResult>;
    fn apply_patch(&self, patch: PatchCheck) -> HarnessResult<PatchApplyResult>;
    fn cleanup_task_worktree(&self, task_id: &TaskId, force: bool) -> HarnessResult<()>;
}

pub trait CommandRunner {
    fn run_validation(&self, spec: CommandSpec) -> HarnessResult<CommandOutput>;
    fn run_shell_escape(&self, spec: CommandSpec) -> HarnessResult<CommandOutput>;
}

#[derive(Debug, Clone)]
pub struct GitWorkspaceManager {
    repo_root: PathBuf,
    worktree_root: PathBuf,
}

impl GitWorkspaceManager {
    pub fn new(
        repo_root: impl Into<PathBuf>,
        worktree_root: impl Into<PathBuf>,
    ) -> HarnessResult<Self> {
        let repo_root = repo_root.into();
        let worktree_root = worktree_root.into();
        if !worktree_root.is_absolute() {
            return Err(HarnessError::InvalidConfig(
                "worktree root must be absolute".to_string(),
            ));
        }
        Ok(Self {
            repo_root,
            worktree_root,
        })
    }

    pub fn for_current_dir() -> HarnessResult<Self> {
        let repo_root = discover_repo_root_from(None)?;
        let repo_name = repo_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("repo");
        let parent = repo_root.parent().ok_or_else(|| {
            HarnessError::InvalidConfig("repository root has no parent directory".to_string())
        })?;
        let worktree_root = parent
            .join(".harness-worktrees")
            .join(format!("{repo_name}-{}", stable_path_hash(&repo_root)));
        Self::new(repo_root, worktree_root)
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    pub fn dirty_summary(&self, repo_root: &str) -> HarnessResult<Option<String>> {
        let status = git_output(["status", "--porcelain=v1"], Path::new(repo_root))?;
        if status.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(status))
        }
    }
}

impl Default for GitWorkspaceManager {
    fn default() -> Self {
        Self::for_current_dir().unwrap_or_else(|_| Self {
            repo_root: PathBuf::from("."),
            worktree_root: std::env::temp_dir().join("harness-worktrees"),
        })
    }
}

impl WorkspaceManager for GitWorkspaceManager {
    fn discover_repo_root(&self, repo: Option<&str>) -> HarnessResult<String> {
        discover_repo_root_from(repo).map(path_to_string)
    }

    fn source_is_dirty(&self, repo_root: &str) -> HarnessResult<bool> {
        let status = git_output(["status", "--porcelain=v1"], Path::new(repo_root))?;
        Ok(!status.trim().is_empty())
    }

    fn resolve_base_commit(
        &self,
        repo_root: &str,
        base_ref: Option<&str>,
    ) -> HarnessResult<String> {
        resolve_commit(Path::new(repo_root), base_ref.unwrap_or("HEAD"))
    }

    fn ensure_task_worktree(&self, request: WorktreeRequest) -> HarnessResult<WorktreeInfo> {
        let expected_branch = task_branch(&request.task_id);
        if let Some(recorded) = request.recorded {
            let info = self.verify_recorded_worktree(&request.repo_root, &recorded)?;
            if info.branch != expected_branch {
                return Err(HarnessError::Conflict(format!(
                    "recorded worktree branch mismatch for task {}: expected {}, found {}",
                    request.task_id.as_str(),
                    expected_branch,
                    info.branch
                )));
            }
            return Ok(info);
        }

        let repo_root = PathBuf::from(&request.repo_root);
        let worktree_root = PathBuf::from(&request.worktree_root);
        if !worktree_root.is_absolute() {
            return Err(HarnessError::InvalidConfig(
                "worktree root must be absolute".to_string(),
            ));
        }

        let repo_root_canonical = canonicalize_existing(&repo_root)?;
        let worktree_path = worktree_root.join(format!("task_{}", request.task_id.as_str()));
        refuse_inside_repo(&repo_root_canonical, &worktree_path)?;
        if worktree_path.exists() {
            return Err(HarnessError::Conflict(format!(
                "worktree path already exists and was not recorded: {}",
                worktree_path.display()
            )));
        }

        fs::create_dir_all(&worktree_root).map_err(io_error("create worktree root"))?;
        let base_ref = request.base_ref.unwrap_or_else(|| "HEAD".to_string());
        let base_commit = resolve_commit(&repo_root, &base_ref)?;
        let branch = expected_branch;

        git_output(
            [
                "worktree",
                "add",
                "-b",
                branch.as_str(),
                path_arg(&worktree_path).as_str(),
                base_commit.as_str(),
            ],
            &repo_root,
        )?;

        let head = resolve_commit(&worktree_path, "HEAD")?;
        Ok(WorktreeInfo {
            path: path_to_string(worktree_path),
            branch,
            base_ref,
            base_commit,
            head,
        })
    }

    fn verify_recorded_worktree(
        &self,
        repo_root: &str,
        recorded: &RecordedWorktree,
    ) -> HarnessResult<WorktreeInfo> {
        let repo_root = PathBuf::from(repo_root);
        let worktree_path = PathBuf::from(&recorded.path);
        let registered = registered_worktree(&repo_root, &worktree_path)?;
        let branch = registered.branch.ok_or_else(|| {
            HarnessError::Conflict(format!(
                "recorded worktree is detached: {}",
                worktree_path.display()
            ))
        })?;

        if branch != recorded.branch {
            return Err(HarnessError::Conflict(format!(
                "recorded worktree branch mismatch: expected {}, found {}",
                recorded.branch, branch
            )));
        }

        verify_same_repository(&repo_root, &worktree_path)?;
        let head = resolve_commit(&worktree_path, "HEAD")?;
        if let Some(expected) = &recorded.last_seen_head {
            if expected != &head {
                return Err(HarnessError::Conflict(format!(
                    "recorded worktree HEAD mismatch: expected {expected}, found {head}"
                )));
            }
        }

        Ok(WorktreeInfo {
            path: recorded.path.clone(),
            branch,
            base_ref: recorded
                .base_ref
                .clone()
                .unwrap_or_else(|| "HEAD".to_string()),
            base_commit: recorded.base_commit.clone().unwrap_or_else(|| head.clone()),
            head,
        })
    }

    fn capture_diff(&self, worktree_path: &str, _run_id: &RunId) -> HarnessResult<String> {
        let unstaged = git_output(["diff", "--binary"], Path::new(worktree_path))?;
        let staged = git_output(["diff", "--cached", "--binary"], Path::new(worktree_path))?;
        let untracked = untracked_allowed_diffs(Path::new(worktree_path))?;
        Ok(join_diffs([staged, unstaged, untracked]))
    }

    fn check_patch(&self, patch: PatchCheck) -> HarnessResult<PatchCheckResult> {
        let validation = validate_patch_safety(&patch.diff, &patch_validation_config(&patch))?;
        let stderr = command_stdin(
            &validation.apply_check.program,
            &validation.apply_check.args,
            Path::new(&validation.apply_check.cwd),
            &patch.diff,
        )?;
        Ok(PatchCheckResult {
            files_changed: validation.files_changed,
            stderr,
        })
    }

    fn apply_patch(&self, patch: PatchCheck) -> HarnessResult<PatchApplyResult> {
        let check = self.check_patch(patch.clone())?;
        let validation = validate_patch_safety(&patch.diff, &patch_validation_config(&patch))?;
        let stderr = command_stdin(
            &validation.apply.program,
            &validation.apply.args,
            Path::new(&validation.apply.cwd),
            &patch.diff,
        )?;
        Ok(PatchApplyResult { check, stderr })
    }

    fn cleanup_task_worktree(&self, task_id: &TaskId, force: bool) -> HarnessResult<()> {
        let worktree_path = self
            .worktree_root
            .join(format!("task_{}", task_id.as_str()));
        if !worktree_path.exists() {
            return Ok(());
        }

        let status = git_output(["status", "--porcelain=v1"], &worktree_path)?;
        if !force && !status.trim().is_empty() {
            return Err(HarnessError::Conflict(format!(
                "refusing to remove dirty worktree {}",
                worktree_path.display()
            )));
        }

        if force {
            git_output(
                [
                    "worktree",
                    "remove",
                    "--force",
                    path_arg(&worktree_path).as_str(),
                ],
                &self.repo_root,
            )?;
        } else {
            git_output(
                ["worktree", "remove", path_arg(&worktree_path).as_str()],
                &self.repo_root,
            )?;
        }
        Ok(())
    }
}

impl CommandRunner for GitWorkspaceManager {
    fn run_validation(&self, spec: CommandSpec) -> HarnessResult<CommandOutput> {
        run_command(spec)
    }

    fn run_shell_escape(&self, spec: CommandSpec) -> HarnessResult<CommandOutput> {
        let spec = CommandSpec {
            cwd: path_to_string(&self.repo_root),
            ..spec
        };
        run_command(spec)
    }
}

fn discover_repo_root_from(repo: Option<&str>) -> HarnessResult<PathBuf> {
    let cwd = repo.map(PathBuf::from).unwrap_or(
        std::env::current_dir()
            .map_err(|err| HarnessError::External(format!("read current directory: {err}")))?,
    );
    let output = git_output(["rev-parse", "--show-toplevel"], &cwd)?;
    Ok(PathBuf::from(output.trim()))
}

fn resolve_commit(repo_root: &Path, reference: &str) -> HarnessResult<String> {
    let rev = format!("{reference}^{{commit}}");
    Ok(
        git_output(["rev-parse", "--verify", rev.as_str()], repo_root)?
            .trim()
            .to_string(),
    )
}

fn git_output<const N: usize>(args: [&str; N], cwd: &Path) -> HarnessResult<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| HarnessError::External(format!("spawn git: {err}")))?;

    if output.status.success() {
        String::from_utf8(output.stdout)
            .map_err(|err| HarnessError::External(format!("git output was not utf-8: {err}")))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(HarnessError::External(format!(
            "git failed in {}: {}",
            cwd.display(),
            stderr.trim()
        )))
    }
}

fn command_stdin(program: &str, args: &[String], cwd: &Path, stdin: &str) -> HarnessResult<String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| HarnessError::External(format!("spawn {program}: {err}")))?;

    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(stdin.as_bytes())
        .map_err(io_error("write git stdin"))?;

    let output = child.wait_with_output().map_err(io_error("wait for git"))?;
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.status.success() {
        Ok(stderr)
    } else {
        Err(HarnessError::External(format!(
            "{program} failed in {}: {}",
            cwd.display(),
            stderr.trim()
        )))
    }
}

fn git_output_allow_exit<const N: usize>(
    args: [&str; N],
    cwd: &Path,
    allowed_exit_codes: &[i32],
) -> HarnessResult<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| HarnessError::External(format!("spawn git: {err}")))?;

    if output.status.success()
        || output
            .status
            .code()
            .is_some_and(|code| allowed_exit_codes.contains(&code))
    {
        String::from_utf8(output.stdout)
            .map_err(|err| HarnessError::External(format!("git output was not utf-8: {err}")))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(HarnessError::External(format!(
            "git failed in {}: {}",
            cwd.display(),
            stderr.trim()
        )))
    }
}

fn run_command(spec: CommandSpec) -> HarnessResult<CommandOutput> {
    let started = Instant::now();
    let sanitized_env = DefaultEnvironmentSanitizer::new().sanitize(&spec.env);
    let mut command = Command::new(&spec.shell_path);
    command
        .env_clear()
        .arg("-c")
        .arg(&spec.command)
        .current_dir(&spec.cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match &spec.stdin {
        CommandStdin::Null => {
            command.stdin(Stdio::null());
        }
        CommandStdin::Bytes(_) => {
            command.stdin(Stdio::piped());
        }
    };

    for (key, value) in &sanitized_env {
        command.env(key, value);
    }

    #[cfg(unix)]
    if spec.kill_process_group_on_timeout {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command.spawn().map_err(|err| {
        HarnessError::External(format!("spawn command {:?}: {err}", spec.command))
    })?;

    if let CommandStdin::Bytes(bytes) = &spec.stdin {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(bytes).map_err(io_error("write stdin"))?;
        }
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| HarnessError::External("stdout was not captured".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| HarnessError::External("stderr was not captured".to_string()))?;
    let max_output_bytes = spec.max_output_bytes;
    let stdout_handle = thread::spawn(move || read_limited(stdout, max_output_bytes));
    let stderr_handle = thread::spawn(move || read_limited(stderr, max_output_bytes));

    let deadline = Instant::now() + Duration::from_secs(spec.timeout_seconds);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(io_error("wait for command"))? {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            terminate_child(&mut child, spec.kill_process_group_on_timeout);
            break child.wait().map_err(io_error("wait after timeout"))?;
        }
        thread::sleep(Duration::from_millis(10));
    };

    let stdout = stdout_handle
        .join()
        .map_err(|_| HarnessError::External("stdout reader panicked".to_string()))??;
    let stderr = stderr_handle
        .join()
        .map_err(|_| HarnessError::External("stderr reader panicked".to_string()))??;
    let truncated_bytes = stdout.truncated_bytes + stderr.truncated_bytes;

    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&stdout.bytes).to_string(),
        stderr: String::from_utf8_lossy(&stderr.bytes).to_string(),
        exit_code: status.code(),
        duration_ms: started.elapsed().as_millis(),
        timed_out,
        truncated: stdout.truncated || stderr.truncated,
        truncated_bytes,
    })
}

fn terminate_child(child: &mut std::process::Child, kill_process_group: bool) {
    #[cfg(unix)]
    if kill_process_group {
        let group = format!("-{}", child.id());
        let _ = Command::new("kill")
            .args(["-TERM", group.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        thread::sleep(Duration::from_millis(50));
        let _ = Command::new("kill")
            .args(["-KILL", group.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        return;
    }

    let _ = child.kill();
}

#[derive(Debug)]
struct LimitedRead {
    bytes: Vec<u8>,
    truncated: bool,
    truncated_bytes: u64,
}

fn read_limited(mut reader: impl Read, max_bytes: u64) -> HarnessResult<LimitedRead> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut truncated_bytes = 0;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(io_error("read command output"))?;
        if read == 0 {
            break;
        }

        let remaining = max_bytes.saturating_sub(bytes.len() as u64) as usize;
        if remaining == 0 {
            truncated = true;
            truncated_bytes += read as u64;
        } else if remaining >= read {
            bytes.extend_from_slice(&buffer[..read]);
        } else {
            bytes.extend_from_slice(&buffer[..remaining]);
            truncated = true;
            truncated_bytes += (read - remaining) as u64;
        }
    }

    Ok(LimitedRead {
        bytes,
        truncated,
        truncated_bytes,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ListedWorktree {
    path: PathBuf,
    head: Option<String>,
    branch: Option<String>,
}

fn registered_worktree(repo_root: &Path, worktree_path: &Path) -> HarnessResult<ListedWorktree> {
    let listed = parse_worktree_list(&git_output(["worktree", "list", "--porcelain"], repo_root)?);
    let wanted = canonicalize_existing(worktree_path)?;
    listed
        .into_iter()
        .find(|entry| canonicalize_existing(&entry.path).ok().as_ref() == Some(&wanted))
        .ok_or_else(|| HarnessError::NotFound {
            kind: "worktree",
            id: worktree_path.display().to_string(),
        })
}

fn parse_worktree_list(output: &str) -> Vec<ListedWorktree> {
    let mut result = Vec::new();
    let mut current: Option<ListedWorktree> = None;
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                result.push(entry);
            }
            current = Some(ListedWorktree {
                path: PathBuf::from(path),
                head: None,
                branch: None,
            });
        } else if let Some(head) = line.strip_prefix("HEAD ") {
            if let Some(entry) = current.as_mut() {
                entry.head = Some(head.to_string());
            }
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if let Some(entry) = current.as_mut() {
                entry.branch = Some(branch.to_string());
            }
        }
    }
    if let Some(entry) = current {
        result.push(entry);
    }
    result
}

fn verify_same_repository(repo_root: &Path, worktree_path: &Path) -> HarnessResult<()> {
    let expected = git_output(["rev-parse", "--git-common-dir"], repo_root)?;
    let actual = git_output(["rev-parse", "--git-common-dir"], worktree_path)?;
    let expected = canonicalize_git_dir(repo_root, expected.trim())?;
    let actual = canonicalize_git_dir(worktree_path, actual.trim())?;
    if expected == actual {
        Ok(())
    } else {
        Err(HarnessError::Conflict(format!(
            "worktree {} belongs to a different git repository",
            worktree_path.display()
        )))
    }
}

fn canonicalize_git_dir(cwd: &Path, value: &str) -> HarnessResult<PathBuf> {
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    canonicalize_existing(path)
}

fn canonicalize_existing(path: impl AsRef<Path>) -> HarnessResult<PathBuf> {
    path.as_ref().canonicalize().map_err(|err| {
        HarnessError::External(format!("canonicalize {}: {err}", path.as_ref().display()))
    })
}

fn refuse_inside_repo(repo_root: &Path, worktree_path: &Path) -> HarnessResult<()> {
    let absolute = if worktree_path.is_absolute() {
        worktree_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(io_error("read current directory"))?
            .join(worktree_path)
    };
    if absolute.starts_with(repo_root) {
        Err(HarnessError::InvalidConfig(format!(
            "worktree path must not be inside source repository: {}",
            absolute.display()
        )))
    } else {
        Ok(())
    }
}

fn patch_validation_config(patch: &PatchCheck) -> PatchValidationConfig {
    PatchValidationConfig {
        worktree_path: patch.worktree_path.clone(),
        max_patch_bytes: patch.max_patch_bytes,
        max_files_changed: patch.max_files_changed,
    }
}

fn untracked_allowed_diffs(worktree_path: &Path) -> HarnessResult<String> {
    let output = git_output(
        ["ls-files", "--others", "--exclude-standard", "-z"],
        worktree_path,
    )?;
    let mut diffs = Vec::new();
    for relative in output.split('\0').filter(|path| !path.is_empty()) {
        let full_path = worktree_path.join(relative);
        if !full_path.is_file() {
            continue;
        }

        let diff = git_output_allow_exit(
            [
                "diff",
                "--no-index",
                "--binary",
                "--",
                "/dev/null",
                relative,
            ],
            worktree_path,
            &[1],
        )?;
        let config = PatchValidationConfig {
            worktree_path: path_to_string(worktree_path),
            max_patch_bytes: u64::MAX,
            max_files_changed: u32::MAX,
        };
        if validate_patch_safety(&diff, &config).is_ok() {
            diffs.push(diff);
        }
    }
    Ok(join_diffs(diffs))
}

fn join_diffs(diffs: impl IntoIterator<Item = String>) -> String {
    let mut joined = String::new();
    for diff in diffs {
        if diff.is_empty() {
            continue;
        }
        if !joined.is_empty() && !joined.ends_with('\n') {
            joined.push('\n');
        }
        joined.push_str(&diff);
    }
    joined
}

fn task_branch(task_id: &TaskId) -> String {
    format!("harness/task_{}", task_id.as_str())
}

fn stable_path_hash(path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    path.display().to_string().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn path_to_string(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().to_string()
}

fn path_arg(path: &Path) -> String {
    path_to_string(path)
}

fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> HarnessError {
    move |err| HarnessError::External(format!("{context}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const TASK_ID: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_ID: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn discovers_repo_root_and_dirty_state() {
        let repo = test_repo();
        let nested = repo.path().join("a/b");
        fs::create_dir_all(&nested).unwrap();
        let manager = manager_for(&repo);

        let root = manager
            .discover_repo_root(Some(nested.to_str().unwrap()))
            .unwrap();
        assert_eq!(
            PathBuf::from(root).canonicalize().unwrap(),
            repo.path().canonicalize().unwrap()
        );
        assert!(
            !manager
                .source_is_dirty(repo.path().to_str().unwrap())
                .unwrap()
        );

        fs::write(repo.path().join("dirty.txt"), "dirty\n").unwrap();
        assert!(
            manager
                .source_is_dirty(repo.path().to_str().unwrap())
                .unwrap()
        );
    }

    #[test]
    fn creates_reuses_verifies_and_removes_worktree() {
        let repo = test_repo();
        let manager = manager_for(&repo);
        let task_id = TaskId::parse(TASK_ID).unwrap();
        let base_commit = manager
            .resolve_base_commit(repo.path().to_str().unwrap(), Some("HEAD"))
            .unwrap();

        let info = manager
            .ensure_task_worktree(WorktreeRequest {
                repo_root: path_to_string(repo.path()),
                worktree_root: path_to_string(manager.worktree_root()),
                task_id: task_id.clone(),
                base_ref: Some("HEAD".to_string()),
                recorded: None,
            })
            .unwrap();

        assert_eq!(info.branch, format!("harness/task_{TASK_ID}"));
        assert_eq!(info.base_commit, base_commit);
        assert!(!PathBuf::from(&info.path).starts_with(repo.path()));

        let recorded = RecordedWorktree {
            path: info.path.clone(),
            branch: info.branch.clone(),
            base_ref: Some(info.base_ref.clone()),
            base_commit: Some(info.base_commit.clone()),
            last_seen_head: Some(info.head.clone()),
        };
        let verified = manager
            .verify_recorded_worktree(repo.path().to_str().unwrap(), &recorded)
            .unwrap();
        assert_eq!(verified, info);

        manager.cleanup_task_worktree(&task_id, false).unwrap();
        assert!(!PathBuf::from(info.path).exists());
    }

    #[test]
    fn recorded_reuse_requires_task_branch() {
        let repo = test_repo();
        let manager = manager_for(&repo);
        let task_id = TaskId::parse(TASK_ID).unwrap();
        let other_task_id = TaskId::parse("task_01BRZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let info = manager
            .ensure_task_worktree(WorktreeRequest {
                repo_root: path_to_string(repo.path()),
                worktree_root: path_to_string(manager.worktree_root()),
                task_id: other_task_id.clone(),
                base_ref: None,
                recorded: None,
            })
            .unwrap();

        let err = manager
            .ensure_task_worktree(WorktreeRequest {
                repo_root: path_to_string(repo.path()),
                worktree_root: path_to_string(manager.worktree_root()),
                task_id,
                base_ref: None,
                recorded: Some(RecordedWorktree {
                    path: info.path,
                    branch: info.branch,
                    base_ref: Some(info.base_ref),
                    base_commit: Some(info.base_commit),
                    last_seen_head: Some(info.head),
                }),
            })
            .unwrap_err();

        assert!(matches!(err, HarnessError::Conflict(_)));
        manager.cleanup_task_worktree(&other_task_id, true).unwrap();
    }

    #[test]
    fn refuses_unrecorded_existing_worktree_path() {
        let repo = test_repo();
        let manager = manager_for(&repo);
        let task_id = TaskId::parse(TASK_ID).unwrap();
        fs::create_dir_all(manager.worktree_root().join(format!("task_{TASK_ID}"))).unwrap();

        let err = manager
            .ensure_task_worktree(WorktreeRequest {
                repo_root: path_to_string(repo.path()),
                worktree_root: path_to_string(manager.worktree_root()),
                task_id,
                base_ref: None,
                recorded: None,
            })
            .unwrap_err();
        assert!(matches!(err, HarnessError::Conflict(_)));
    }

    #[test]
    fn cleanup_refuses_dirty_worktree_unless_forced() {
        let repo = test_repo();
        let manager = manager_for(&repo);
        let task_id = TaskId::parse(TASK_ID).unwrap();
        let info = manager
            .ensure_task_worktree(WorktreeRequest {
                repo_root: path_to_string(repo.path()),
                worktree_root: path_to_string(manager.worktree_root()),
                task_id: task_id.clone(),
                base_ref: None,
                recorded: None,
            })
            .unwrap();

        fs::write(Path::new(&info.path).join("untracked.txt"), "dirty\n").unwrap();
        let err = manager.cleanup_task_worktree(&task_id, false).unwrap_err();
        assert!(matches!(err, HarnessError::Conflict(_)));
        manager.cleanup_task_worktree(&task_id, true).unwrap();
        assert!(!Path::new(&info.path).exists());
    }

    #[test]
    fn captures_tracked_and_untracked_allowed_new_file_diffs() {
        let repo = test_repo();
        let manager = manager_for(&repo);
        let task_id = TaskId::parse(TASK_ID).unwrap();
        let info = manager
            .ensure_task_worktree(WorktreeRequest {
                repo_root: path_to_string(repo.path()),
                worktree_root: path_to_string(manager.worktree_root()),
                task_id,
                base_ref: None,
                recorded: None,
            })
            .unwrap();

        fs::write(Path::new(&info.path).join("file.txt"), "changed\n").unwrap();
        fs::write(Path::new(&info.path).join("created.txt"), "created\n").unwrap();
        let diff = manager
            .capture_diff(&info.path, &RunId::parse(RUN_ID).unwrap())
            .unwrap();
        assert!(diff.contains("changed"));
        assert!(diff.contains("diff --git a/created.txt b/created.txt"));

        run_git(
            repo.path(),
            ["worktree", "remove", "--force", info.path.as_str()],
        );
        let info = manager
            .ensure_task_worktree(WorktreeRequest {
                repo_root: path_to_string(repo.path()),
                worktree_root: path_to_string(manager.worktree_root()),
                task_id: TaskId::parse("task_01BRZ3NDEKTSV4RRFFQ69G5FAV").unwrap(),
                base_ref: None,
                recorded: None,
            })
            .unwrap();
        let check = manager
            .check_patch(PatchCheck {
                worktree_path: info.path.clone(),
                diff: diff.clone(),
                max_patch_bytes: 10_000,
                max_files_changed: 2,
            })
            .unwrap();
        assert_eq!(check.files_changed, vec!["created.txt", "file.txt"]);
        manager
            .apply_patch(PatchCheck {
                worktree_path: info.path.clone(),
                diff,
                max_patch_bytes: 10_000,
                max_files_changed: 2,
            })
            .unwrap();
        assert_eq!(
            fs::read_to_string(Path::new(&info.path).join("file.txt")).unwrap(),
            "changed\n"
        );
        assert_eq!(
            fs::read_to_string(Path::new(&info.path).join("created.txt")).unwrap(),
            "created\n"
        );
    }

    #[test]
    fn check_patch_uses_full_patch_safety_validation() {
        let repo = test_repo();
        let manager = manager_for(&repo);
        let info = manager
            .ensure_task_worktree(WorktreeRequest {
                repo_root: path_to_string(repo.path()),
                worktree_root: path_to_string(manager.worktree_root()),
                task_id: TaskId::parse(TASK_ID).unwrap(),
                base_ref: None,
                recorded: None,
            })
            .unwrap();

        let err = manager
            .check_patch(PatchCheck {
                worktree_path: info.path,
                diff: "diff --git a/file.txt b/file.txt\ndeleted file mode 100644\n--- a/file.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-initial\n".to_string(),
                max_patch_bytes: 10_000,
                max_files_changed: 2,
            })
            .unwrap_err();

        assert!(matches!(err, HarnessError::SecurityPolicy(_)));
    }

    #[test]
    fn command_runner_captures_output_exit_duration_and_truncation() {
        let repo = test_repo();
        let manager = manager_for(&repo);
        let output = manager
            .run_validation(CommandSpec {
                command: "printf abcdef; printf err >&2; exit 7".to_string(),
                cwd: path_to_string(repo.path()),
                shell_path: "/bin/sh".to_string(),
                env: BTreeMap::new(),
                timeout_seconds: 5,
                max_output_bytes: 3,
                stdin: CommandStdin::Null,
                kill_process_group_on_timeout: true,
            })
            .unwrap();

        assert_eq!(output.stdout, "abc");
        assert_eq!(output.stderr, "err");
        assert_eq!(output.exit_code, Some(7));
        assert!(!output.timed_out);
        assert!(output.truncated);
        assert_eq!(output.truncated_bytes, 3);
        assert!(output.duration_ms < 5_000);
    }

    #[test]
    fn command_runner_times_out() {
        let repo = test_repo();
        let manager = manager_for(&repo);
        let output = manager
            .run_validation(CommandSpec {
                command: "/bin/sleep 2".to_string(),
                cwd: path_to_string(repo.path()),
                shell_path: "/bin/sh".to_string(),
                env: BTreeMap::new(),
                timeout_seconds: 0,
                max_output_bytes: 100,
                stdin: CommandStdin::Null,
                kill_process_group_on_timeout: true,
            })
            .unwrap();

        assert!(output.timed_out);
    }

    #[test]
    fn command_runner_clears_parent_env_and_sanitizes_spec_env() {
        let repo = test_repo();
        let manager = manager_for(&repo);
        let output = manager
            .run_validation(CommandSpec {
                command: "printf '%s:%s:%s' \"$NO_COLOR\" \"$SECRET_TOKEN\" \"$HOME\"".to_string(),
                cwd: path_to_string(repo.path()),
                shell_path: "/bin/sh".to_string(),
                env: BTreeMap::from([
                    ("NO_COLOR".to_string(), "kept".to_string()),
                    ("SECRET_TOKEN".to_string(), "dropped".to_string()),
                ]),
                timeout_seconds: 5,
                max_output_bytes: 100,
                stdin: CommandStdin::Null,
                kill_process_group_on_timeout: true,
            })
            .unwrap();

        assert_eq!(output.stdout, "kept::");
    }

    #[test]
    fn shell_escape_runs_from_repo_root() {
        let repo = test_repo();
        let manager = manager_for(&repo);
        let other = TempDir::new().unwrap();
        let output = manager
            .run_shell_escape(CommandSpec {
                command: "pwd".to_string(),
                cwd: path_to_string(other.path()),
                shell_path: "/bin/sh".to_string(),
                env: BTreeMap::new(),
                timeout_seconds: 5,
                max_output_bytes: 1_000,
                stdin: CommandStdin::Null,
                kill_process_group_on_timeout: true,
            })
            .unwrap();

        assert_eq!(
            PathBuf::from(output.stdout.trim()).canonicalize().unwrap(),
            repo.path().canonicalize().unwrap()
        );
    }

    fn test_repo() -> TempDir {
        let temp = TempDir::new().unwrap();
        run_git(temp.path(), ["init", "-b", "main"]);
        fs::write(temp.path().join("file.txt"), "initial\n").unwrap();
        run_git(temp.path(), ["add", "."]);
        run_git(
            temp.path(),
            [
                "-c",
                "user.name=Harness Test",
                "-c",
                "user.email=harness@example.invalid",
                "commit",
                "-m",
                "initial",
            ],
        );
        temp
    }

    fn manager_for(repo: &TempDir) -> GitWorkspaceManager {
        let unique_root = repo.path().parent().unwrap().join(format!(
            "worktrees-{}",
            repo.path().file_name().unwrap().to_string_lossy()
        ));
        GitWorkspaceManager::new(repo.path(), unique_root).unwrap()
    }

    fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
