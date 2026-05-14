use crate::error::HarnessResult;
use crate::runtime::{
    CancellationToken, CommandExit, CommandResult, CommandStatus, TuiRuntimeEvent,
};
use crate::security::{DefaultEnvironmentSanitizer, EnvironmentSanitizer};
use crate::workspace::{CommandOutput, CommandSpec, CommandStdin};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SHELL_PATH: &str = "/bin/sh";
const SHELL_ESCAPE_TIMEOUT_SECONDS: u64 = 3600;
const SHELL_ESCAPE_MAX_OUTPUT_BYTES: u64 = 1024 * 1024;

pub trait ShellEscapeRunner {
    fn run_shell_escape(
        &self,
        spec: CommandSpec,
        cancellation: &dyn CancellationToken,
    ) -> HarnessResult<ShellEscapeOutput>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellEscapeOutput {
    pub output: CommandOutput,
    pub cancelled: bool,
}

impl ShellEscapeOutput {
    pub fn completed(output: CommandOutput) -> Self {
        Self {
            output,
            cancelled: false,
        }
    }

    pub fn cancelled(output: CommandOutput) -> Self {
        Self {
            output,
            cancelled: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DefaultShellEscapeRunner {
    repo_root: PathBuf,
}

impl DefaultShellEscapeRunner {
    pub fn for_repo_root(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
        }
    }
}

impl ShellEscapeRunner for DefaultShellEscapeRunner {
    fn run_shell_escape(
        &self,
        spec: CommandSpec,
        cancellation: &dyn CancellationToken,
    ) -> HarnessResult<ShellEscapeOutput> {
        let spec = CommandSpec {
            cwd: self.repo_root.to_string_lossy().into_owned(),
            ..spec
        };
        run_cancellable_command(spec, cancellation)
    }
}

pub fn default_shell_escape_runner() -> HarnessResult<DefaultShellEscapeRunner> {
    Ok(DefaultShellEscapeRunner::for_repo_root(
        discover_repo_root()?
    ))
}

pub fn run_shell_escape<R>(
    command: &str,
    runner: &R,
    cancellation: &dyn CancellationToken,
) -> Vec<TuiRuntimeEvent>
where
    R: ShellEscapeRunner,
{
    let command = command.trim();
    if command.is_empty() {
        return vec![TuiRuntimeEvent::Failed(
            "shell escape command cannot be empty".to_string(),
        )];
    }

    let spec = shell_escape_spec(command);
    if cancellation.is_cancelled() {
        return vec![TuiRuntimeEvent::CancelAcknowledged { next_command: None }];
    }
    match runner.run_shell_escape(spec, cancellation) {
        Ok(output) if output.cancelled => {
            vec![TuiRuntimeEvent::CancelAcknowledged { next_command: None }]
        }
        Ok(output) => output_events(output.output),
        Err(err) => vec![TuiRuntimeEvent::Failed(format!(
            "failed to run shell escape: {err}"
        ))],
    }
}

pub fn shell_escape_spec(command: &str) -> CommandSpec {
    CommandSpec {
        command: command.to_string(),
        cwd: current_dir_string(),
        shell_path: SHELL_PATH.to_string(),
        env: std::env::vars().collect::<BTreeMap<_, _>>(),
        timeout_seconds: SHELL_ESCAPE_TIMEOUT_SECONDS,
        max_output_bytes: SHELL_ESCAPE_MAX_OUTPUT_BYTES,
        stdin: CommandStdin::Null,
        kill_process_group_on_timeout: true,
    }
}

fn output_events(output: CommandOutput) -> Vec<TuiRuntimeEvent> {
    let mut events = Vec::new();
    if !output.stdout.is_empty() {
        events.push(TuiRuntimeEvent::Stdout(output.stdout.clone()));
    }
    if !output.stderr.is_empty() {
        events.push(TuiRuntimeEvent::Stderr(output.stderr.clone()));
    }
    if output.truncated {
        events.push(TuiRuntimeEvent::Stderr(format!(
            "shell escape output truncated by {} byte(s)",
            output.truncated_bytes
        )));
    }

    let exit = if output.timed_out {
        events.push(TuiRuntimeEvent::Stderr(
            "shell escape timed out".to_string(),
        ));
        CommandExit::new(
            CommandStatus::Failed,
            1,
            Some("shell escape timed out".to_string()),
        )
    } else if output.exit_code == Some(0) {
        CommandExit::success()
    } else {
        let code_text = output
            .exit_code
            .map_or_else(|| "signal".to_string(), |code| code.to_string());
        events.push(TuiRuntimeEvent::Stderr(format!(
            "shell escape exited with {code_text}"
        )));
        CommandExit::new(
            CommandStatus::Failed,
            output.exit_code.and_then(exit_code_u8).unwrap_or(1),
            Some(format!("shell escape exited with {code_text}")),
        )
    };

    events.push(TuiRuntimeEvent::CommandFinished(CommandResult::new(exit)));
    events
}

fn exit_code_u8(code: i32) -> Option<u8> {
    u8::try_from(code).ok()
}

fn current_dir_string() -> String {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .into_owned()
}

fn discover_repo_root() -> HarnessResult<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|err| crate::HarnessError::External(format!("read current directory: {err}")))?;
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| crate::HarnessError::External(format!("discover repository root: {err}")))?;
    if output.status.success() {
        Ok(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    } else {
        Err(crate::HarnessError::External(format!(
            "discover repository root: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn run_cancellable_command(
    spec: CommandSpec,
    cancellation: &dyn CancellationToken,
) -> HarnessResult<ShellEscapeOutput> {
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
        crate::HarnessError::External(format!("spawn shell escape {:?}: {err}", spec.command))
    })?;

    if let CommandStdin::Bytes(bytes) = &spec.stdin
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin
            .write_all(bytes)
            .map_err(io_error("write shell escape stdin"))?;
    }

    let stdout = child.stdout.take().ok_or_else(|| {
        crate::HarnessError::External("shell escape stdout was not captured".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        crate::HarnessError::External("shell escape stderr was not captured".to_string())
    })?;
    let max_output_bytes = spec.max_output_bytes;
    let stdout_handle = thread::spawn(move || read_limited(stdout, max_output_bytes));
    let stderr_handle = thread::spawn(move || read_limited(stderr, max_output_bytes));

    let deadline = Instant::now() + Duration::from_secs(spec.timeout_seconds);
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(io_error("wait for shell escape"))?
        {
            break status;
        }
        if cancellation.is_cancelled() {
            cancelled = true;
            terminate_child(&mut child, spec.kill_process_group_on_timeout);
            break child
                .wait()
                .map_err(io_error("wait after shell escape cancellation"))?;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            terminate_child(&mut child, spec.kill_process_group_on_timeout);
            break child
                .wait()
                .map_err(io_error("wait after shell escape timeout"))?;
        }
        thread::sleep(Duration::from_millis(10));
    };

    let stdout = stdout_handle.join().map_err(|_| {
        crate::HarnessError::External("shell escape stdout reader panicked".to_string())
    })??;
    let stderr = stderr_handle.join().map_err(|_| {
        crate::HarnessError::External("shell escape stderr reader panicked".to_string())
    })??;
    let truncated_bytes = stdout.truncated_bytes + stderr.truncated_bytes;
    let output = CommandOutput {
        stdout: String::from_utf8_lossy(&stdout.bytes).to_string(),
        stderr: String::from_utf8_lossy(&stderr.bytes).to_string(),
        exit_code: status.code(),
        duration_ms: started.elapsed().as_millis(),
        timed_out,
        truncated: stdout.truncated || stderr.truncated,
        truncated_bytes,
    };

    Ok(if cancelled {
        ShellEscapeOutput::cancelled(output)
    } else {
        ShellEscapeOutput::completed(output)
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
            .map_err(io_error("read shell escape output"))?;
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

fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> crate::HarnessError {
    move |err| crate::HarnessError::External(format!("{context}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn shell_escape_spec_uses_sanitized_runner_contract_knobs() {
        let spec = shell_escape_spec("printf ok");

        assert_eq!(spec.command, "printf ok");
        assert_eq!(spec.shell_path, "/bin/sh");
        assert_eq!(spec.stdin, CommandStdin::Null);
        assert_eq!(spec.timeout_seconds, 3600);
        assert_eq!(spec.max_output_bytes, 1024 * 1024);
        assert!(spec.kill_process_group_on_timeout);
    }

    #[test]
    fn shell_escape_runner_receives_parent_env_for_workspace_sanitization() {
        let runner = RecordingRunner::new(CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(0),
            duration_ms: 1,
            timed_out: false,
            truncated: false,
            truncated_bytes: 0,
        });

        let cancellation = AtomicBool::new(false);
        run_shell_escape("env", &runner, &cancellation);

        let spec = runner.spec.borrow();
        let spec = spec.as_ref().unwrap();
        assert_eq!(spec.stdin, CommandStdin::Null);
        assert!(spec.env.keys().any(|key| key == "PATH"));
    }

    #[test]
    fn shell_escape_redacts_and_sanitizes_when_appended_to_transcript() {
        let output = CommandOutput {
            stdout: "hello\x1b[2J OPENAI_API_KEY=sk-test-secret\n".to_string(),
            stderr: String::new(),
            exit_code: Some(7),
            duration_ms: 1,
            timed_out: false,
            truncated: false,
            truncated_bytes: 0,
        };
        let events = output_events(output);
        let mut app = crate::tui::TuiAppState::default();
        for event in events {
            app.append_runtime_event(event);
        }

        let transcript = app.transcript.entries().next().unwrap();
        assert!(transcript.text.contains("hello"));
        assert!(!transcript.text.contains("\x1b"));
        assert!(!transcript.text.contains("sk-test-secret"));
        assert!(transcript.secret_redacted);
        assert!(
            app.transcript
                .entries()
                .any(|entry| entry.text.contains("shell escape exited with 7"))
        );
    }

    #[test]
    fn shell_escape_nonzero_renders_failed_command_finished_event() {
        let events = output_events(CommandOutput {
            stdout: String::new(),
            stderr: "bad\n".to_string(),
            exit_code: Some(42),
            duration_ms: 1,
            timed_out: false,
            truncated: false,
            truncated_bytes: 0,
        });

        assert!(events.iter().any(|event| matches!(
            event,
            TuiRuntimeEvent::Stderr(text) if text.contains("shell escape exited with 42")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TuiRuntimeEvent::CommandFinished(result)
                if result.exit.status == CommandStatus::Failed && result.exit.code() == 42
        )));
    }

    #[cfg(unix)]
    #[test]
    fn shell_escape_cancellation_kills_process_group_and_acknowledges_promptly() {
        let repo = tempfile::tempdir().unwrap();
        let runner = DefaultShellEscapeRunner::for_repo_root(repo.path());
        let cancellation = AtomicBool::new(false);
        let started = Instant::now();
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(50));
                cancellation.store(true, std::sync::atomic::Ordering::SeqCst);
            });
            let events = run_shell_escape("sleep 5", &runner, &cancellation);
            assert!(started.elapsed() < Duration::from_secs(1));
            assert!(events.iter().any(|event| {
                matches!(
                    event,
                    TuiRuntimeEvent::CancelAcknowledged { next_command: None }
                )
            }));
        });
    }

    struct RecordingRunner {
        output: CommandOutput,
        spec: RefCell<Option<CommandSpec>>,
    }

    impl RecordingRunner {
        fn new(output: CommandOutput) -> Self {
            Self {
                output,
                spec: RefCell::new(None),
            }
        }
    }

    impl ShellEscapeRunner for RecordingRunner {
        fn run_shell_escape(
            &self,
            spec: CommandSpec,
            _cancellation: &dyn CancellationToken,
        ) -> HarnessResult<ShellEscapeOutput> {
            *self.spec.borrow_mut() = Some(spec);
            Ok(ShellEscapeOutput::completed(self.output.clone()))
        }
    }
}
