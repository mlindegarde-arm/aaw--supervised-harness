#![cfg(unix)]
#![allow(dead_code)]

#[path = "support/binary.rs"]
mod binary;
#[path = "support/fake_providers.rs"]
mod fake_providers;
#[path = "support/fixtures.rs"]
mod fixtures;
#[path = "support/pty.rs"]
mod pty;

use binary::BinaryHarness;
use fake_providers::{FakeOllamaServer, FakeOpenAiServer};
use fixtures::{FixtureKind, create_or_reuse_fixture, inject_fake_provider_config};
use pty::{PtyHarness, PtySize, assert_terminal_cleanup};
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const SHORT: Duration = Duration::from_secs(3);
const MEDIUM: Duration = Duration::from_secs(8);
const WIDE: PtySize = PtySize::new(120, 32);
const NARROW: PtySize = PtySize::new(72, 24);

#[test]
fn tty_route_opens_tui_and_exits_with_terminal_cleanup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    BinaryHarness::new().init_repo_json(&fixture.path);

    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    let screen = tui.wait_for_text("Composer", SHORT).unwrap();
    assert!(screen.text.contains("Transcript"), "{}", screen.text);
    assert!(screen.text.contains(">"), "{}", screen.text);

    tui.press_ctrl_d().unwrap();
    let exit = tui.wait_for_exit(SHORT).unwrap();

    assert_terminal_cleanup(&exit);
}

#[test]
fn non_tty_no_command_uses_fallback_interactive_shell() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    BinaryHarness::new().init_repo_json(&fixture.path);

    let mut child = Command::new(env!("CARGO_BIN_EXE_harness"))
        .current_dir(&fixture.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn harness fallback");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"version\nexit\n")
        .unwrap();
    let output = child.wait_with_output().expect("wait fallback");

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains("interactive mode"), "{stdout}");
    assert!(stdout.contains(&format!("harness {}", env!("CARGO_PKG_VERSION"))));
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn invalid_command_recovers_and_shell_env_is_sanitized() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    BinaryHarness::new().init_repo_json(&fixture.path);

    let mut tui = harness_pty(&fixture.path, WIDE)
        .env("OPENAI_API_KEY", "sk-testsecret1234567890abcdef")
        .env("ARM_OPENAI_API_KEY", "sk-armtestsecret1234567890abcdef")
        .env("HARNESS_OLLAMA_BASE_URL", "http://127.0.0.1:9")
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text("/task nope").unwrap();
    tui.press_enter().unwrap();
    let screen = tui
        .wait_for_text("Choose a task subcommand", SHORT)
        .unwrap();
    assert!(screen.text.contains("Composer"), "{}", screen.text);
    tui.press_ctrl_u().unwrap();

    tui.type_text("!env").unwrap();
    tui.press_enter().unwrap();
    let screen = tui.wait_for_text("PATH=", MEDIUM).unwrap();
    assert!(!screen.text.contains("OPENAI_API_KEY"), "{}", screen.text);
    assert!(
        !screen.text.contains("ARM_OPENAI_API_KEY"),
        "{}",
        screen.text
    );
    assert!(
        !screen.text.contains("HARNESS_OLLAMA_BASE_URL"),
        "{}",
        screen.text
    );

    tui.press_ctrl_u().unwrap();
    tui.press_ctrl_u().unwrap();
    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn shell_escape_output_is_redacted_and_terminal_controls_are_sanitized() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    BinaryHarness::new().init_repo_json(&fixture.path);

    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text("!printf 'hello\\033[2J OPENAI_API_KEY=sk-testsecret1234567890abcdef\\n'")
        .unwrap();
    tui.press_enter().unwrap();
    let screen = tui
        .wait_for_text("command finished: complete", MEDIUM)
        .unwrap();

    assert!(screen.text.contains("Composer"), "{}", screen.text);
    assert!(screen.text.contains("hello"), "{}", screen.text);
    assert!(!screen.text.contains("sk-testsecret"), "{}", screen.text);
    assert!(screen.text.contains("[REDACTED"), "{}", screen.text);

    tui.press_ctrl_u().unwrap();
    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn task_tab_renders_subcommand_suggestions() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    BinaryHarness::new().init_repo_json(&fixture.path);

    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text("/task ").unwrap();
    tui.press_tab().unwrap();
    let screen = tui.wait_for_text("Create a task", SHORT).unwrap();
    assert!(screen.text.contains("List tasks"), "{}", screen.text);
    assert!(screen.text.contains("Run a task once"), "{}", screen.text);

    tui.press_ctrl_u().unwrap();
    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn plain_task_list_renders_prompt_first_compatibility_warning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    BinaryHarness::new().init_repo_json(&fixture.path);

    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text("task list").unwrap();
    let screen = tui
        .wait_for_text("Use /task to run harness commands", SHORT)
        .unwrap();
    assert!(screen.text.contains("> task list"), "{}", screen.text);

    tui.press_ctrl_u().unwrap();
    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn seeded_resume_task_id_completion_renders_status_and_title_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let binary = BinaryHarness::new().current_dir(&fixture.path);
    binary.init_repo_json(&fixture.path);
    create_task(&binary, "TUI PTY completion Alpha", "true");
    create_task(&binary, "TUI PTY completion Beta", "true");

    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text("/resume task_").unwrap();
    tui.press_tab().unwrap();
    let screen = tui.wait_for_text("TUI PTY completion", SHORT).unwrap();
    assert!(screen.text.contains("task_"), "{}", screen.text);
    assert!(screen.text.contains("ready"), "{}", screen.text);

    tui.press_ctrl_u().unwrap();
    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn seeded_ticket_id_completion_renders_status_and_question_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let ticket_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(
        local_model,
        ["STUCK\nreason: need guidance\nquestion: Which TUI PTY ticket path should be used?"],
    );
    let openai = FakeOpenAiServer::scripted(ticket_model, [("resp_ticket_unused", "unused")]);
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");
    binary.init_repo_json(&fixture.path);
    inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());
    let task_id = create_task(
        &binary,
        "TUI PTY ticket completion",
        &cargo_validation_command(),
    );
    let ticket_id = create_stuck_ticket(&binary, &task_id, local_model);

    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text("/ticket get ticket_").unwrap();
    tui.press_tab().unwrap();
    let screen = tui.wait_for_text(&ticket_id, SHORT).unwrap();
    assert!(screen.text.contains("ticket_"), "{}", screen.text);
    assert!(screen.text.contains("open"), "{}", screen.text);
    assert!(screen.text.contains(&ticket_id), "{}", screen.text);

    tui.press_ctrl_u().unwrap();
    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn seeded_objective_id_completion_renders_status_and_title_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let planner_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(local_model, [success_patch()]);
    let openai = FakeOpenAiServer::scripted(
        planner_model,
        [("resp_objective_completion_plan", objective_planner_json())],
    );
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");
    binary.init_repo_json(&fixture.path);
    inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());
    let objective_id = create_objective(&binary);

    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text("/objective get objective_").unwrap();
    tui.press_tab().unwrap();
    let screen = tui.wait_for_text(&objective_id, SHORT).unwrap();
    assert!(screen.text.contains("objective_"), "{}", screen.text);
    assert!(screen.text.contains("ready"), "{}", screen.text);
    assert!(screen.text.contains(&objective_id), "{}", screen.text);

    tui.press_ctrl_u().unwrap();
    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn down_enter_and_tab_insert_selected_suggestions() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    BinaryHarness::new().init_repo_json(&fixture.path);

    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text("/task ").unwrap();
    tui.wait_for_text("Create a task", SHORT).unwrap();
    tui.press_down().unwrap();
    tui.press_enter().unwrap();
    let screen = tui.wait_for_text("> /task cleanup", SHORT).unwrap();
    assert!(screen.text.contains("> /task cleanup"), "{}", screen.text);

    tui.press_ctrl_u().unwrap();
    tui.wait_for_absence("> /task cleanup", SHORT).unwrap();
    tui.type_text("/task ").unwrap();
    tui.wait_for_text("Create a task", SHORT).unwrap();
    tui.press_down().unwrap();
    tui.press_down().unwrap();
    tui.press_tab().unwrap();
    let screen = tui.wait_for_text("> /task create", SHORT).unwrap();
    assert!(screen.text.contains("> /task create"), "{}", screen.text);

    tui.press_ctrl_u().unwrap();
    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn foreground_supervise_streams_progress_and_reenables_composer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let ticket_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(local_model, [success_patch()]);
    let openai = FakeOpenAiServer::scripted(ticket_model, [("resp_tui_unused", "unused")]);
    let task_id = seeded_task(&fixture.path, &ollama, &openai);

    let command =
        format!("/supervise {task_id} --max-attempts 1 --model {local_model} --max-cycles 0");
    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text(&command).unwrap();
    tui.press_enter().unwrap();
    let running = tui
        .wait_for_text("Composer (command running)", MEDIUM)
        .unwrap();
    assert!(running.text.contains("running"), "{}", running.text);

    let finished = tui
        .wait_for_text("complete after supervision", Duration::from_secs(20))
        .unwrap();
    assert!(
        finished.text.contains("RunTask") || finished.text.contains("running task"),
        "{}",
        finished.text
    );
    assert!(finished.text.contains("Composer"), "{}", finished.text);
    assert!(
        !finished.text.contains("Composer (command running)"),
        "{}",
        finished.text
    );

    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn prompt_first_objective_renders_dashboard_lifecycle() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let planner_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(local_model, [success_patch()]);
    let openai = FakeOpenAiServer::scripted(
        planner_model,
        [("resp_tui_objective_plan", objective_planner_json())],
    );
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");
    binary.init_repo_json(&fixture.path);
    inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());

    let mut tui = harness_pty(&fixture.path, WIDE)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key")
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text("Create a small Rust maintenance objective")
        .unwrap();
    tui.press_enter().unwrap();
    let planning = tui.wait_for_text("Objective Dashboard", MEDIUM).unwrap();
    assert!(planning.text.contains("Remote"), "{}", planning.text);

    let completed = tui
        .wait_for_text("objective complete", Duration::from_secs(20))
        .unwrap();
    assert!(
        completed.text.contains("Objective Dashboard"),
        "{}",
        completed.text
    );
    assert!(completed.text.contains("Validation"), "{}", completed.text);
    assert!(completed.text.contains("complete"), "{}", completed.text);

    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn ctrl_c_during_foreground_supervise_requests_cancellation_and_shows_next_command() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let ticket_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(local_model, [success_patch()]);
    let openai = FakeOpenAiServer::scripted(ticket_model, [("resp_tui_unused", "unused")]);
    let task_id = seeded_task_with_validation(&fixture.path, &ollama, &openai, "/bin/sleep 2");

    let command =
        format!("/supervise {task_id} --max-attempts 1 --model {local_model} --max-cycles 0");
    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Composer", SHORT).unwrap();

    tui.type_text(&command).unwrap();
    tui.press_enter().unwrap();
    tui.wait_for_text("running task", MEDIUM).unwrap();
    tui.press_ctrl_c().unwrap();
    let cancelling = tui.wait_for_text("cancelling", SHORT).unwrap();
    assert!(
        cancelling.text.contains("Composer (command running)"),
        "{}",
        cancelling.text
    );

    let cancelled = tui
        .wait_for_text(
            "cancellation acknowledged; resume with",
            Duration::from_secs(8),
        )
        .unwrap();
    assert!(
        cancelled.text.contains("harness task get"),
        "{}",
        cancelled.text
    );
    assert!(
        !cancelled.text.contains("Composer (command running)"),
        "{}",
        cancelled.text
    );

    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

#[test]
fn narrow_and_wide_layouts_render_without_obvious_overlap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    BinaryHarness::new().init_repo_json(&fixture.path);

    for size in [NARROW, WIDE] {
        let mut tui = harness_pty(&fixture.path, size)
            .spawn([] as [&str; 0])
            .unwrap();
        let screen = tui.wait_for_text("Composer", SHORT).unwrap();
        assert!(screen.text.contains("Transcript"), "{}", screen.text);
        assert!(screen.text.contains("Tasks"), "{}", screen.text);
        for line in screen.nonblank_lines() {
            assert!(
                line.chars().count() <= size.cols as usize,
                "line exceeded PTY width {size:?}: {line:?}"
            );
        }
        tui.press_ctrl_d().unwrap();
        assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
    }
}

#[test]
fn side_pane_switching_and_transcript_scrolling_work_under_pty() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    BinaryHarness::new().init_repo_json(&fixture.path);

    let mut tui = harness_pty(&fixture.path, WIDE)
        .spawn([] as [&str; 0])
        .unwrap();
    tui.wait_for_text("Tasks", SHORT).unwrap();

    tui.press_ctrl_n().unwrap();
    tui.wait_for_text("Tickets", SHORT).unwrap();
    tui.press_ctrl_p().unwrap();
    tui.wait_for_text("Tasks", SHORT).unwrap();
    tui.press_escape().unwrap();

    tui.type_text("!i=1; while [ $i -le 36 ]; do printf 'scroll_%02d\\n' $i; i=$((i+1)); done")
        .unwrap();
    tui.press_enter().unwrap();
    let bottom = tui
        .wait_for_text("command finished: complete (0)", MEDIUM)
        .unwrap();
    assert!(bottom.text.contains("scroll_36"), "{}", bottom.text);
    assert!(!bottom.text.contains("scroll_12"), "{}", bottom.text);

    tui.press_page_up().unwrap();
    tui.press_page_up().unwrap();
    let scrolled = tui.wait_for_text("scroll_12", SHORT).unwrap();
    assert!(scrolled.text.contains("scroll_12"), "{}", scrolled.text);
    assert!(!scrolled.text.contains("scroll_36"), "{}", scrolled.text);

    tui.press_page_down().unwrap();
    tui.press_page_down().unwrap();
    let restored = tui.wait_for_absence("scroll_12", SHORT).unwrap();
    assert!(restored.text.contains("scroll_36"), "{}", restored.text);

    tui.press_ctrl_d().unwrap();
    assert_terminal_cleanup(&tui.wait_for_exit(SHORT).unwrap());
}

fn harness_pty(fixture: &Path, size: PtySize) -> PtyHarness {
    PtyHarness::new(env!("CARGO_BIN_EXE_harness"))
        .current_dir(fixture)
        .size(size)
}

fn seeded_task(fixture: &Path, ollama: &FakeOllamaServer, openai: &FakeOpenAiServer) -> String {
    seeded_task_with_validation(fixture, ollama, openai, &cargo_validation_command())
}

fn seeded_task_with_validation(
    fixture: &Path,
    ollama: &FakeOllamaServer,
    openai: &FakeOpenAiServer,
    validation: &str,
) -> String {
    let binary = BinaryHarness::new()
        .current_dir(fixture)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");
    binary.init_repo_json(fixture);
    inject_fake_provider_config(fixture, &ollama.base_url(), &openai.base_url());

    create_task(&binary, "TUI PTY supervise", validation)
}

fn create_task(binary: &BinaryHarness, title: &str, validation: &str) -> String {
    let created = binary.run([
        "--output",
        "json",
        "task",
        "create",
        "--title",
        title,
        "--goal",
        "Make the intentionally failing fixture pass",
        "--validation",
        validation,
    ]);
    assert!(
        created.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        created.stdout,
        created.stderr
    );
    let json: Value = serde_json::from_str(created.stdout.trim()).unwrap();
    json["data"]["task_id"].as_str().unwrap().to_string()
}

fn create_stuck_ticket(binary: &BinaryHarness, task_id: &str, model: &str) -> String {
    let ran = binary.run([
        "--output",
        "json",
        "task",
        "run",
        task_id,
        "--max-attempts",
        "1",
        "--model",
        model,
    ]);
    assert_eq!(
        ran.status.code(),
        Some(10),
        "stdout:\n{}\nstderr:\n{}",
        ran.stdout,
        ran.stderr
    );
    let json: Value = serde_json::from_str(ran.stdout.trim()).unwrap();
    assert_eq!(json["status"], "stuck", "{json:#?}");
    json["data"]["ticket_id"].as_str().unwrap().to_string()
}

fn create_objective(binary: &BinaryHarness) -> String {
    let started = binary.run([
        "--output",
        "json",
        "objective",
        "start",
        "--prompt",
        "Create a small Rust maintenance objective",
    ]);
    assert!(
        started.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        started.stdout,
        started.stderr
    );
    let json: Value = serde_json::from_str(started.stdout.trim()).unwrap();
    assert_eq!(json["status"], "complete", "{json:#?}");
    json["data"]["objective_id"].as_str().unwrap().to_string()
}

fn cargo_validation_command() -> String {
    let rust_tool_dir = Path::new(env!("CARGO")).parent().unwrap().to_string_lossy();
    format!("PATH={rust_tool_dir}:/usr/bin:/bin {} test", env!("CARGO"))
}

fn success_patch() -> &'static str {
    "```diff\ndiff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,5 +1,5 @@\n pub fn add(left: i32, right: i32) -> i32 {\n-    left - right\n+    left + right\n }\n \n #[cfg(test)]\n\n```"
}

fn objective_planner_json() -> String {
    serde_json::json!({
        "schema_version": 1,
        "objective": {
            "title": "Maintain Rust project",
            "summary": "Run a small validation-backed Rust maintenance objective.",
            "acceptance_criteria": ["filtered cargo test passes"],
            "validation_commands": ["cargo test validate_project"]
        },
        "tasks": [
            {
                "task_key": "validate_project",
                "title": "Validate project",
                "goal": "Confirm the Rust project remains valid.",
                "validation": ["cargo test validate_project"],
                "depends_on": [],
                "owned_paths": ["src"],
                "parallel_group": "validation"
            }
        ],
        "risks": [],
        "final_verification": ["cargo test validate_project"]
    })
    .to_string()
}
