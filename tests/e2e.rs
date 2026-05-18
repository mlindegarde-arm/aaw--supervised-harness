#[path = "support/binary.rs"]
mod binary;
#[path = "support/fake_providers.rs"]
mod fake_providers;
#[path = "support/fixtures.rs"]
mod fixtures;

use binary::BinaryHarness;
use fake_providers::{FakeOllamaServer, FakeOpenAiServer};
use fixtures::{
    FixtureKind, assert_fake_provider_config, assert_local_provider_url, create_fixture,
    create_or_reuse_fixture, inject_fake_provider_config,
    inject_fake_provider_config_with_openai_policy,
};
use harness::config::{ConfigPaths, init_repo, load_config};
use harness::domain::{
    Artifact, ArtifactId, Attempt, AttemptId, AttemptStatus, Event, EventId, EventLevel, Run,
    RunId, RunStatus, Task, TaskId, TaskStatus, Ticket, TicketId, TicketResolution,
    TicketResolutionId, TicketStatus,
};
use harness::patch::{
    OllamaResponse, PatchValidationConfig, parse_ollama_response, validate_patch_safety,
};
use harness::prompts::{ArtifactManifest, build_artifact_manifest, ticket_resolution_text};
use harness::providers::{FakeModelProvider, ModelProvider, ModelRequest, ProviderFuture};
use harness::runtime::{
    CommandExit, CommandResult, CommandRuntime, CommandStatus, JsonSink, OutputMode,
    ResumeTaskOptions, RuntimeOptions, TaskRunOptions, TicketResolveOptions,
};
use harness::security::{DefaultRedactor, Redactor};
use harness::service::{DefaultHarnessService, HarnessService};
use harness::state::{RunUpdate, SqliteTaskStore, TaskStore};
use harness::workspace::{
    CommandOutput, CommandRunner, CommandSpec, GitWorkspaceManager, PatchApplyResult, PatchCheck,
    PatchCheckResult, RecordedWorktree, WorkspaceManager, WorktreeInfo, WorktreeRequest,
};
use harness::{HarnessError, HarnessResult};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

const OWNER: &str = "e2e-owner";
const SECRET: &str = "Authorization: Bearer sk-testsecret1234567890abcdef";

#[test]
fn binary_harness_runner_captures_stdout_stderr_and_exit_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let binary = BinaryHarness::new().current_dir(&fixture.path);

    let init = binary.init_repo(&fixture.path);
    assert!(init.status.success(), "{init:?}");
    assert!(
        init.stdout.contains("\"status\":\"complete\""),
        "{}",
        init.stdout
    );
    assert!(init.stderr.is_empty(), "{}", init.stderr);

    let invalid = binary.run(["definitely-not-a-command"]);
    assert_eq!(invalid.status.code(), Some(2), "{invalid:?}");
    assert!(invalid.stdout.is_empty(), "{}", invalid.stdout);
    assert!(
        invalid.stderr.contains("unknown command"),
        "{}",
        invalid.stderr
    );
}

#[test]
fn binary_harness_fixture_repo_initializes_with_repo_flag() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let binary = BinaryHarness::new();

    let init_json = binary.init_repo_json(&fixture.path);
    assert_eq!(init_json["status"], "complete");
    assert_eq!(init_json["exit_code"], 0);
    assert_eq!(
        PathBuf::from(init_json["data"]["repo_root"].as_str().unwrap()),
        fixture.path.canonicalize().unwrap()
    );
    assert!(fixture.path.join(".harness/config.toml").exists());
    assert!(fixture.path.join(".harness/state.sqlite").exists());
}

#[test]
fn binary_harness_config_injection_points_at_local_fake_providers() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let binary = BinaryHarness::new();
    let ollama = FakeOllamaServer::scripted("binary-local-model", [success_patch()]);
    let openai = FakeOpenAiServer::scripted(
        "binary-ticket-model",
        [("resp_binary_unused", "unused ticket response")],
    );

    binary.init_repo_json(&fixture.path);
    let paths = inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());
    assert!(
        paths.config_file.exists(),
        "{}",
        paths.config_file.display()
    );

    let loaded = load_config(Some(&fixture.path)).expect("load injected config");
    assert_fake_provider_config(&loaded.config);
    assert_eq!(loaded.config.providers.ollama.base_url, ollama.base_url());
    assert_eq!(loaded.config.providers.openai.base_url, openai.base_url());
}

#[test]
fn binary_harness_fake_provider_receives_records_and_scripts_requests() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let ticket_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(local_model, [success_patch()]);
    let openai = FakeOpenAiServer::scripted(
        ticket_model,
        [("resp_binary_001", "ticket response should remain unused")],
    );
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");

    binary.init_repo_json(&fixture.path);
    inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());

    let repo = fixture.path.to_str().unwrap();
    let rust_tool_dir = Path::new(env!("CARGO")).parent().unwrap().to_string_lossy();
    let validation = format!("PATH={rust_tool_dir}:/usr/bin:/bin {} test", env!("CARGO"));
    let created = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "create",
        "--title",
        "Binary fix add",
        "--goal",
        "Make the intentionally failing fixture pass",
        "--validation",
        validation.as_str(),
    ]);
    assert!(created.status.success(), "{created:?}");
    assert!(created.stderr.is_empty(), "{}", created.stderr);
    let task_id = json_str(&created.stdout, "/data/task_id");

    let ran = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "run",
        task_id.as_str(),
        "--max-attempts",
        "1",
        "--model",
        local_model,
    ]);
    assert!(ran.status.success(), "{ran:?}");
    assert!(!ran.stderr.contains("error:"), "{}", ran.stderr);
    let ran_json: Value = serde_json::from_str(ran.stdout.trim()).unwrap();
    assert_eq!(ran_json["status"], "complete");
    assert_eq!(ran_json["exit_code"], 0);

    let ollama_requests = ollama.requests();
    let generate_requests = ollama_requests
        .iter()
        .filter(|request| request.method == "POST" && request.path == "/api/generate")
        .collect::<Vec<_>>();
    assert_eq!(generate_requests.len(), 1, "{ollama_requests:?}");
    let body = generate_requests[0].json_body().unwrap();
    assert_eq!(body["model"], local_model);
    assert!(
        body["prompt"]
            .as_str()
            .is_some_and(|prompt| prompt.contains("Make the intentionally failing fixture pass"))
    );
    assert!(
        openai.requests().is_empty(),
        "task run should not contact ticket provider"
    );
}

#[test]
fn binary_objective_start_supervise_success_emits_strict_json_and_objective_events() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    make_rust_success_fixture_pass(&fixture.path);
    let local_model = "binary-local-model";
    let planner_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(
        local_model,
        [
            "STUCK\nreason: need implementation guidance\nquestion: Which source-level maintenance change should be made?".to_string(),
            passing_add_patch().to_string(),
        ],
    );
    let openai = FakeOpenAiServer::scripted(
        planner_model,
        [
            ("resp_objective_success", objective_planner_json()),
            ("resp_objective_resolver", objective_resolver_json()),
        ],
    );
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");

    binary.init_repo_json(&fixture.path);
    inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());

    let output = binary.run([
        "--output",
        "json",
        "objective",
        "start",
        "--prompt",
        "Create a small Rust maintenance objective",
        "--supervise",
        "--max-worker-attempts",
        "2",
        "--max-cycles",
        "16",
    ]);

    let json = assert_binary_json_contract(&output, "complete", 0);
    assert_eq!(json["data"]["status"], "complete", "{json:#?}");
    assert_eq!(json["data"]["terminal"], true, "{json:#?}");
    assert_eq!(json["data"]["validation"]["status"], "passed", "{json:#?}");
    assert_eq!(json["data"]["validation"]["commands_run"], 1, "{json:#?}");

    let events = assert_objective_ndjson_events(&output.stderr);
    for expected in [
        "objective.planning_started",
        "objective.planning_completed",
        "objective.supervision_started",
        "objective.worker_started",
        "objective.ticket_detected",
        "objective.ticket_resolution_started",
        "objective.ticket_resolution_completed",
        "objective.worker_resumed",
        "objective.worker_completed",
        "objective.validation_passed",
        "objective.completed",
    ] {
        assert!(
            events.iter().any(|event| event["event"] == expected),
            "missing {expected} in {events:#?}"
        );
    }
    assert_eq!(
        provider_requests_for_path(&ollama.requests(), "/api/generate").len(),
        2
    );
    assert_eq!(
        provider_requests_for_path(&openai.requests(), "/responses").len(),
        2
    );
}

#[test]
fn binary_objective_start_supervise_direct_worker_success_emits_strict_json_and_objective_events() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let planner_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(local_model, [success_patch(), success_patch()]);
    let openai = FakeOpenAiServer::scripted(
        planner_model,
        [
            ("resp_objective_success", objective_planner_json()),
            ("resp_objective_resolver", objective_resolver_json()),
        ],
    );
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");

    binary.init_repo_json(&fixture.path);
    inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());

    let output = binary.run([
        "--output",
        "json",
        "objective",
        "start",
        "--prompt",
        "Create a small Rust maintenance objective",
        "--supervise",
        "--max-worker-attempts",
        "2",
        "--max-cycles",
        "16",
    ]);

    let json = assert_binary_json_contract(&output, "complete", 0);
    assert_eq!(json["data"]["status"], "complete", "{json:#?}");
    assert_eq!(json["data"]["terminal"], true, "{json:#?}");
    assert_eq!(json["data"]["validation"]["status"], "passed", "{json:#?}");
    assert_eq!(json["data"]["validation"]["commands_run"], 1, "{json:#?}");

    let events = assert_objective_ndjson_events(&output.stderr);
    for expected in [
        "objective.planning_started",
        "objective.planning_completed",
        "objective.supervision_started",
        "objective.worker_started",
        "objective.worker_completed",
        "objective.validation_passed",
        "objective.completed",
    ] {
        assert!(
            events.iter().any(|event| event["event"] == expected),
            "missing {expected} in {events:#?}"
        );
    }
}

#[test]
fn binary_objective_start_rejects_malformed_planner_output_with_strict_json_contract() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let planner_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(local_model, [success_patch()]);
    let openai = FakeOpenAiServer::scripted(
        planner_model,
        [("resp_objective_malformed", "not valid planner json")],
    );
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");

    binary.init_repo_json(&fixture.path);
    inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());

    let output = binary.run([
        "--output",
        "json",
        "objective",
        "start",
        "--prompt",
        "Create a small Rust maintenance objective",
        "--supervise",
        "--max-worker-attempts",
        "1",
        "--max-cycles",
        "8",
    ]);

    let json = assert_binary_json_contract(&output, "usage", 2);
    assert_eq!(json["data"]["status"], "failed", "{json:#?}");
    assert_eq!(json["data"]["terminal"], true, "{json:#?}");
    let events = assert_objective_ndjson_events(&output.stderr);
    assert_eq!(
        events[0]["event"], "objective.planning_started",
        "{events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event["event"] == "objective.plan_rejected"),
        "{events:#?}"
    );
}

#[test]
fn binary_harness_sanitizes_inherited_provider_override_env() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let ticket_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(local_model, [success_patch()]);
    let openai = FakeOpenAiServer::scripted(
        ticket_model,
        [("resp_binary_unused", "ticket response should remain unused")],
    );
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .inherited_env("HARNESS_OLLAMA_BASE_URL", "http://127.0.0.1:9")
        .inherited_env("HARNESS_OPENAI_BASE_URL", "https://example.invalid/v1")
        .inherited_env("HARNESS_ALLOW_UNTRUSTED_PROVIDER_URL", "false")
        .inherited_env("HARNESS_OLLAMA_DEFAULT_MODEL", "poisoned-local-model")
        .inherited_env("HARNESS_OPENAI_DEFAULT_MODEL", "poisoned-ticket-model")
        .inherited_env("ARM_OPENAI_API_KEY", "poisoned-inherited-key")
        .inherited_env("OPENAI_API_KEY", "poisoned-inherited-key")
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");

    binary.init_repo_json(&fixture.path);
    inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());

    let repo = fixture.path.to_str().unwrap();
    let rust_tool_dir = Path::new(env!("CARGO")).parent().unwrap().to_string_lossy();
    let validation = format!("PATH={rust_tool_dir}:/usr/bin:/bin {} test", env!("CARGO"));
    let created = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "create",
        "--title",
        "Binary env sanitize",
        "--goal",
        "Make the intentionally failing fixture pass",
        "--validation",
        validation.as_str(),
    ]);
    assert!(created.status.success(), "{created:?}");
    assert!(created.stderr.is_empty(), "{}", created.stderr);
    let task_id = json_str(&created.stdout, "/data/task_id");

    let ran = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "run",
        task_id.as_str(),
        "--max-attempts",
        "1",
        "--model",
        local_model,
    ]);
    assert!(ran.status.success(), "{ran:?}");
    assert!(!ran.stderr.contains("error:"), "{}", ran.stderr);
    let ran_json: Value = serde_json::from_str(ran.stdout.trim()).unwrap();
    assert_eq!(ran_json["status"], "complete");
    assert_eq!(ran_json["exit_code"], 0);

    let ollama_requests = ollama.requests();
    let generate_requests = ollama_requests
        .iter()
        .filter(|request| request.method == "POST" && request.path == "/api/generate")
        .collect::<Vec<_>>();
    assert_eq!(generate_requests.len(), 1, "{ollama_requests:?}");
    let body = generate_requests[0].json_body().unwrap();
    assert_eq!(body["model"], local_model);
    assert!(
        openai.requests().is_empty(),
        "task run should not contact ticket provider"
    );
}

#[test]
fn binary_harness_fake_openai_records_ordinal_responses() {
    let openai = FakeOpenAiServer::scripted(
        "binary-ticket-model",
        [
            ("resp_binary_first", "first scripted response"),
            ("resp_binary_second", "second scripted response"),
        ],
    );
    let first: Value = ureq::post(&format!("{}/responses", openai.base_url()))
        .set("Authorization", "Bearer binary-test-key")
        .send_json(json!({
            "model": "binary-ticket-model",
            "instructions": "",
            "input": "question one",
            "stream": false,
            "store": false,
            "max_output_tokens": 128,
            "metadata": {},
        }))
        .unwrap()
        .into_json()
        .unwrap();
    let second: Value = ureq::post(&format!("{}/responses", openai.base_url()))
        .set("Authorization", "Bearer binary-test-key")
        .send_json(json!({
            "model": "binary-ticket-model",
            "instructions": "",
            "input": "question two",
            "stream": false,
            "store": false,
            "max_output_tokens": 128,
            "metadata": {},
        }))
        .unwrap()
        .into_json()
        .unwrap();

    assert_eq!(first["id"], "resp_binary_first");
    assert_eq!(second["id"], "resp_binary_second");
    let requests = openai.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method == "POST" && request.path == "/responses")
            .count(),
        2,
        "{requests:?}"
    );
}

#[test]
fn binary_harness_real_provider_url_guard_rejects_non_local_urls() {
    let result = std::panic::catch_unwind(|| {
        assert_local_provider_url("https://openai-api-proxy.geo.arm.com/api/providers/openai-us/v1")
    });
    assert!(
        result.is_err(),
        "real provider URL should fail local e2e guard"
    );
}

#[test]
fn binary_harness_supervise_success_after_ticket_resolution() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustResumeAfterTicket);
    let local_model = "binary-local-model";
    let ticket_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(
        local_model,
        [
            "STUCK\nreason: need product decision before normalizing input\nquestion: Should normalize trim and lowercase ASCII-compatible input?".to_string(),
            resume_patch().to_string(),
        ],
    );
    let openai = FakeOpenAiServer::scripted(
        ticket_model,
        [(
            "resp_supervise_001",
            format!(
                "Decision: trim surrounding whitespace and lowercase the result.\nDo not leak {SECRET}."
            ),
        )],
    );
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");

    binary.init_repo_json(&fixture.path);
    let paths = inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());
    let repo = fixture.path.to_str().unwrap();
    let validation = cargo_validation_command();
    let goal = format!("Make normalize pass. Evidence includes {SECRET}");

    let created = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "create",
        "--title",
        "Binary supervise normalization",
        "--goal",
        goal.as_str(),
        "--validation",
        validation.as_str(),
    ]);
    let created_json = assert_binary_json_contract(&created, "complete", 0);
    assert!(created.stderr.is_empty(), "{}", created.stderr);
    let task_id = TaskId::parse(created_json["data"]["task_id"].as_str().unwrap()).unwrap();

    let supervised = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "supervise",
        task_id.as_str(),
        "--max-attempts",
        "1",
        "--model",
        local_model,
        "--ticket-model",
        ticket_model,
        "--max-cycles",
        "2",
    ]);
    let supervised_json = assert_binary_json_contract(&supervised, "complete", 0);
    let events = assert_supervise_ndjson_phases(
        &supervised.stderr,
        &["inspect", "run", "inspect", "resolve", "resume", "complete"],
    );
    assert_eq!(supervised_json["data"]["task_id"], task_id.as_str());
    assert_eq!(supervised_json["data"]["cycles"], 1);
    assert!(
        supervised_json["data"]["next_commands"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        supervised_json["data"]["progress_events"]
            .as_array()
            .unwrap()
            .len(),
        events.len()
    );

    let store = SqliteTaskStore::open(&paths.state_file).expect("open sqlite state");
    assert_eq!(
        store.get_task(&task_id).unwrap().status,
        TaskStatus::Complete
    );
    let resolved_tickets = json_string_array(&supervised_json, "/data/resolved_tickets");
    let resolution_ids = json_string_array(&supervised_json, "/data/resolution_ids");
    let run_ids = json_string_array(&supervised_json, "/data/run_ids");
    assert_eq!(resolved_tickets.len(), 1);
    assert_eq!(resolution_ids.len(), 1);
    assert_eq!(run_ids.len(), 2);
    let resolution_id = TicketResolutionId::parse(&resolution_ids[0]).unwrap();
    let resolution = store.get_ticket_resolution(&resolution_id).unwrap();
    assert_eq!(resolution.ticket_id.as_str(), resolved_tickets[0]);
    assert!(resolution.consumed_at.is_some(), "{resolution:?}");
    for run_id in &run_ids {
        let run_id = RunId::parse(run_id).unwrap();
        assert_run_artifacts_have_manifest(&store, &run_id);
    }

    let ollama_requests = ollama.requests();
    let ollama_generate = provider_requests_for_path(&ollama_requests, "/api/generate");
    assert_eq!(ollama_generate.len(), 2, "{ollama_requests:?}");
    let first_prompt = ollama_generate[0]["prompt"].as_str().unwrap();
    let second_prompt = ollama_generate[1]["prompt"].as_str().unwrap();
    assert!(first_prompt.contains("[REDACTED"), "{first_prompt}");
    assert!(!first_prompt.contains("sk-testsecret"), "{first_prompt}");
    assert!(
        second_prompt.contains("ticket_resolutions"),
        "{second_prompt}"
    );
    assert!(second_prompt.contains("[REDACTED"), "{second_prompt}");
    assert!(!second_prompt.contains("sk-testsecret"), "{second_prompt}");

    let openai_requests = openai.requests();
    let openai_responses = provider_requests_for_path(&openai_requests, "/responses");
    assert_eq!(openai_responses.len(), 1, "{openai_requests:?}");
    let ticket_input = openai_responses[0]["input"].as_str().unwrap();
    assert!(ticket_input.contains("Should normalize"), "{ticket_input}");
    assert!(!ticket_input.contains("sk-testsecret"), "{ticket_input}");

    assert_no_secret_in_binary_surfaces(
        &paths,
        &[
            &created.stdout,
            &created.stderr,
            &supervised.stdout,
            &supervised.stderr,
        ],
        &[&ollama_requests, &openai_requests],
    );
}

#[test]
fn binary_harness_supervise_create_completes_with_final_json_and_progress_ndjson() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let ollama = FakeOllamaServer::scripted(local_model, [success_patch()]);
    let openai = FakeOpenAiServer::scripted(
        "binary-ticket-model",
        [("resp_supervise_create_unused", "unused")],
    );
    let binary = BinaryHarness::new().current_dir(&fixture.path);

    binary.init_repo_json(&fixture.path);
    let paths = inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());
    let repo = fixture.path.to_str().unwrap();
    let validation = cargo_validation_command();

    let supervised = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "supervise",
        "--create",
        "--title",
        "Binary supervise create add",
        "--goal",
        "Make the intentionally failing fixture pass",
        "--validation",
        validation.as_str(),
        "--max-attempts",
        "1",
        "--model",
        local_model,
        "--max-cycles",
        "0",
    ]);
    let supervised_json = assert_binary_json_contract(&supervised, "complete", 0);
    assert_supervise_ndjson_phases(&supervised.stderr, &["inspect", "run", "complete"]);
    let task_id = TaskId::parse(supervised_json["data"]["task_id"].as_str().unwrap()).unwrap();
    assert!(
        supervised_json["data"]["next_commands"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let store = SqliteTaskStore::open(&paths.state_file).expect("open sqlite state");
    assert_eq!(
        store.get_task(&task_id).unwrap().status,
        TaskStatus::Complete
    );
    let run_ids = json_string_array(&supervised_json, "/data/run_ids");
    assert_eq!(run_ids.len(), 1);
    assert_run_artifacts_have_manifest(&store, &RunId::parse(&run_ids[0]).unwrap());
    assert_eq!(
        provider_requests_for_path(&ollama.requests(), "/api/generate").len(),
        1
    );
    assert!(
        provider_requests_for_path(&openai.requests(), "/responses").is_empty(),
        "supervise --create success should not contact ticket provider"
    );
}

#[test]
fn binary_harness_supervise_resumes_resolved_unconsumed_ticket_without_new_openai_call() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustResumeAfterTicket);
    let local_model = "binary-local-model";
    let ticket_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(
        local_model,
        [
            "STUCK\nreason: need product decision before normalizing input\nquestion: Should normalize trim and lowercase ASCII-compatible input?".to_string(),
            resume_patch().to_string(),
        ],
    );
    let openai = FakeOpenAiServer::scripted(
        ticket_model,
        [(
            "resp_interrupted_001",
            "Decision: trim surrounding whitespace and lowercase the result.",
        )],
    );
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");

    binary.init_repo_json(&fixture.path);
    let paths = inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());
    let repo = fixture.path.to_str().unwrap();
    let validation = cargo_validation_command();

    let created = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "create",
        "--title",
        "Binary supervise interrupted normalization",
        "--goal",
        "Make normalize pass after a resolved ticket",
        "--validation",
        validation.as_str(),
    ]);
    let created_json = assert_binary_json_contract(&created, "complete", 0);
    let task_id = TaskId::parse(created_json["data"]["task_id"].as_str().unwrap()).unwrap();

    let stuck = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "run",
        task_id.as_str(),
        "--max-attempts",
        "1",
        "--model",
        local_model,
    ]);
    let stuck_json = assert_binary_json_contract(&stuck, "stuck", 10);
    let ticket_id = TicketId::parse(stuck_json["data"]["ticket_id"].as_str().unwrap()).unwrap();

    let resolved = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "ticket",
        "resolve",
        ticket_id.as_str(),
        "--model",
        ticket_model,
    ]);
    assert_binary_json_contract(&resolved, "complete", 0);
    assert_eq!(
        provider_requests_for_path(&openai.requests(), "/responses").len(),
        1
    );

    let supervised = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "supervise",
        task_id.as_str(),
        "--ticket",
        ticket_id.as_str(),
        "--max-attempts",
        "1",
        "--model",
        local_model,
        "--ticket-model",
        ticket_model,
    ]);
    let supervised_json = assert_binary_json_contract(&supervised, "complete", 0);
    assert_supervise_ndjson_phases(&supervised.stderr, &["inspect", "resume", "complete"]);
    assert_eq!(
        provider_requests_for_path(&openai.requests(), "/responses").len(),
        1,
        "resolved unconsumed ticket must not be resolved again"
    );
    assert_eq!(
        provider_requests_for_path(&ollama.requests(), "/api/generate").len(),
        2
    );

    let store = SqliteTaskStore::open(&paths.state_file).expect("open sqlite state");
    assert_eq!(
        store.get_task(&task_id).unwrap().status,
        TaskStatus::Complete
    );
    assert!(
        store
            .list_ticket_resolutions(&ticket_id)
            .unwrap()
            .iter()
            .all(|resolution| resolution.consumed_at.is_some())
    );
    assert_eq!(
        supervised_json["data"]["resolved_tickets"][0],
        ticket_id.as_str()
    );
}

#[test]
fn binary_harness_supervise_stops_after_escalation_limit_without_extra_openai_call() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustValidationFailsThenStuck);
    let local_model = "binary-local-model";
    let ticket_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(
        local_model,
        [
            "STUCK\nreason: first local block\nquestion: What should change first?",
            "STUCK\nreason: second local block\nquestion: What should change second?",
        ],
    );
    let openai = FakeOpenAiServer::scripted(ticket_model, [("resp_supervise_cap", "Try once.")]);
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");

    binary.init_repo_json(&fixture.path);
    let paths = inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());
    let repo = fixture.path.to_str().unwrap();
    let validation = cargo_validation_command();
    let created = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "create",
        "--title",
        "Binary supervise cap",
        "--goal",
        "Stop after one escalation cycle",
        "--validation",
        validation.as_str(),
    ]);
    let created_json = assert_binary_json_contract(&created, "complete", 0);
    let task_id = TaskId::parse(created_json["data"]["task_id"].as_str().unwrap()).unwrap();

    let supervised = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "supervise",
        task_id.as_str(),
        "--max-attempts",
        "1",
        "--model",
        local_model,
        "--ticket-model",
        ticket_model,
        "--max-cycles",
        "1",
    ]);
    let supervised_json = assert_binary_json_contract(&supervised, "stuck", 10);
    assert_nonzero_next_commands(&supervised_json);
    assert_supervise_ndjson_phases(
        &supervised.stderr,
        &[
            "inspect", "run", "inspect", "resolve", "resume", "inspect", "stuck",
        ],
    );
    assert_eq!(
        provider_requests_for_path(&openai.requests(), "/responses").len(),
        1,
        "supervisor must not resolve the second ticket after the cycle cap"
    );
    assert_eq!(
        provider_requests_for_path(&ollama.requests(), "/api/generate").len(),
        2
    );
    let store = SqliteTaskStore::open(&paths.state_file).expect("open sqlite state");
    assert_eq!(store.get_task(&task_id).unwrap().status, TaskStatus::Stuck);
    assert_eq!(
        store.latest_run_for_task(&task_id).unwrap().unwrap().status,
        RunStatus::Stuck
    );
}

#[test]
fn binary_harness_supervise_missing_provider_readiness_exits_20() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustResumeAfterTicket);
    let local_model = "binary-local-model";
    let ticket_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(
        local_model,
        ["STUCK\nreason: need escalation\nquestion: What should the worker do?"],
    );
    let openai = FakeOpenAiServer::scripted(ticket_model, [("resp_readiness_unused", "unused")]);
    let binary = BinaryHarness::new().current_dir(&fixture.path);

    binary.init_repo_json(&fixture.path);
    inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());
    let repo = fixture.path.to_str().unwrap();
    let validation = cargo_validation_command();
    let created = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "create",
        "--title",
        "Binary supervise readiness",
        "--goal",
        "Escalate without provider credentials",
        "--validation",
        validation.as_str(),
    ]);
    let created_json = assert_binary_json_contract(&created, "complete", 0);
    let task_id = created_json["data"]["task_id"].as_str().unwrap();

    let supervised = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "supervise",
        task_id,
        "--max-attempts",
        "1",
        "--model",
        local_model,
        "--ticket-model",
        ticket_model,
        "--max-cycles",
        "1",
    ]);
    let supervised_json = assert_binary_json_contract(&supervised, "doctor_failed", 20);
    assert_nonzero_next_commands(&supervised_json);
    assert_supervise_ndjson_phases(
        &supervised.stderr,
        &["inspect", "run", "inspect", "resolve", "failed"],
    );
    assert!(
        provider_requests_for_path(&openai.requests(), "/responses").is_empty(),
        "missing API key should fail readiness before contacting OpenAI-compatible HTTP"
    );
}

#[test]
fn binary_harness_supervise_security_block_exits_30_for_untrusted_openai_url() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustResumeAfterTicket);
    let local_model = "binary-local-model";
    let ticket_model = "binary-ticket-model";
    let ollama = FakeOllamaServer::scripted(
        local_model,
        ["STUCK\nreason: need escalation\nquestion: What should the worker do?"],
    );
    let openai = FakeOpenAiServer::scripted(ticket_model, [("resp_security_unused", "unused")]);
    let binary = BinaryHarness::new()
        .current_dir(&fixture.path)
        .env("ARM_OPENAI_API_KEY", "binary-test-key")
        .env("OPENAI_API_KEY", "binary-test-key");

    binary.init_repo_json(&fixture.path);
    inject_fake_provider_config_with_openai_policy(
        &fixture.path,
        &ollama.base_url(),
        &openai.base_url(),
        false,
    );
    let repo = fixture.path.to_str().unwrap();
    let validation = cargo_validation_command();
    let created = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "create",
        "--title",
        "Binary supervise security",
        "--goal",
        "Escalate with an untrusted credentialed provider URL",
        "--validation",
        validation.as_str(),
    ]);
    let created_json = assert_binary_json_contract(&created, "complete", 0);
    let task_id = created_json["data"]["task_id"].as_str().unwrap();

    let supervised = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "supervise",
        task_id,
        "--max-attempts",
        "1",
        "--model",
        local_model,
        "--ticket-model",
        ticket_model,
        "--max-cycles",
        "1",
    ]);
    let supervised_json = assert_binary_json_contract(&supervised, "security_blocked", 30);
    assert_nonzero_next_commands(&supervised_json);
    assert_supervise_ndjson_phases(
        &supervised.stderr,
        &["inspect", "run", "inspect", "resolve", "failed"],
    );
    assert!(
        provider_requests_for_path(&openai.requests(), "/responses").is_empty(),
        "provider URL policy should block before contacting OpenAI-compatible HTTP"
    );
}

#[test]
fn binary_harness_supervise_provider_error_redacts_sentinel_everywhere() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_or_reuse_fixture(temp.path(), FixtureKind::RustSuccess);
    let local_model = "binary-local-model";
    let ollama = FakeOllamaServer::http_error(
        local_model,
        500,
        &format!("local provider failed while handling {SECRET}"),
    );
    let openai = FakeOpenAiServer::scripted(
        "binary-ticket-model",
        [("resp_provider_error_unused", "unused")],
    );
    let binary = BinaryHarness::new().current_dir(&fixture.path);

    binary.init_repo_json(&fixture.path);
    let paths = inject_fake_provider_config(&fixture.path, &ollama.base_url(), &openai.base_url());
    let repo = fixture.path.to_str().unwrap();
    let validation = cargo_validation_command();
    let goal = format!("Make the intentionally failing fixture pass. Evidence includes {SECRET}");
    let created = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "task",
        "create",
        "--title",
        "Binary supervise provider error redaction",
        "--goal",
        goal.as_str(),
        "--validation",
        validation.as_str(),
    ]);
    let created_json = assert_binary_json_contract(&created, "complete", 0);
    let task_id = created_json["data"]["task_id"].as_str().unwrap();

    let supervised = binary.run([
        "--repo",
        repo,
        "--output",
        "json",
        "supervise",
        task_id,
        "--max-attempts",
        "1",
        "--model",
        local_model,
        "--max-cycles",
        "0",
    ]);
    let supervised_json = assert_binary_json_contract(&supervised, "stuck", 10);
    assert_nonzero_next_commands(&supervised_json);
    assert_supervise_ndjson_phases(&supervised.stderr, &["inspect", "run", "inspect", "stuck"]);

    let ollama_requests = ollama.requests();
    let openai_requests = openai.requests();
    assert_eq!(
        provider_requests_for_path(&ollama_requests, "/api/generate").len(),
        1
    );
    assert!(
        provider_requests_for_path(&openai_requests, "/responses").is_empty(),
        "max-cycles 0 should prevent escalation after the redacted provider error"
    );
    assert_no_secret_in_binary_surfaces(
        &paths,
        &[
            &created.stdout,
            &created.stderr,
            &supervised.stdout,
            &supervised.stderr,
        ],
        &[&ollama_requests, &openai_requests],
    );
}

#[test]
fn init_and_offline_doctor_acceptance_create_private_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_fixture(temp.path(), FixtureKind::RustSuccess);

    let cli_init = run_harness_cli(&fixture.path, ["init", "--output", "json"]);
    assert!(cli_init.status.success(), "{cli_init:?}");
    assert!(cli_init.stderr.is_empty(), "{}", cli_init.stderr);
    let cli_init_json: Value = serde_json::from_str(cli_init.stdout.trim()).unwrap();
    assert_eq!(cli_init_json["status"], "complete");
    assert_eq!(cli_init_json["exit_code"], 0);
    let state_file = PathBuf::from(cli_init_json["data"]["state_file"].as_str().unwrap());
    let config_file = PathBuf::from(cli_init_json["data"]["config_file"].as_str().unwrap());
    assert!(state_file.exists(), "{}", state_file.display());
    assert!(config_file.exists(), "{}", config_file.display());

    let doctor = run_harness_cli(&fixture.path, ["doctor", "--offline", "--output", "json"]);
    assert!(doctor.status.success(), "{doctor:?}");
    assert!(!doctor.stderr.contains("error:"), "{}", doctor.stderr);
    let doctor_json: Value = serde_json::from_str(doctor.stdout.trim()).unwrap();
    assert_eq!(doctor_json["status"], "complete");
    assert_eq!(doctor_json["exit_code"], 0);
    assert_eq!(doctor_json["data"]["mode"], "offline");

    let created = run_harness_cli(
        &fixture.path,
        [
            "--output",
            "json",
            "task",
            "create",
            "--title",
            "CLI task",
            "--goal",
            "Exercise the shipped task command wiring",
            "--validation",
            "cargo test",
        ],
    );
    assert!(created.status.success(), "{created:?}");
    assert!(created.stderr.is_empty(), "{}", created.stderr);
    let created_json: Value = serde_json::from_str(created.stdout.trim()).unwrap();
    assert_eq!(created_json["status"], "complete");
    assert_eq!(created_json["exit_code"], 0);

    let listed = run_harness_cli(&fixture.path, ["--output", "json", "task", "list"]);
    assert!(listed.status.success(), "{listed:?}");
    assert!(listed.stderr.is_empty(), "{}", listed.stderr);
    let listed_json: Value = serde_json::from_str(listed.stdout.trim()).unwrap();
    assert_eq!(listed_json["status"], "complete");
    assert!(
        listed_json["data"]["tasks"]
            .as_array()
            .is_some_and(|tasks| tasks.len() == 1)
    );

    let init = init_repo(Some(&fixture.path)).expect("init repo");
    let store = SqliteTaskStore::open(&init.paths.state_file).expect("open sqlite state");
    let loaded = load_config(Some(&fixture.path)).expect("load config");

    assert!(!init.config_created);
    assert!(init.paths.config_file.exists());
    assert!(init.paths.state_file.exists());
    assert_eq!(loaded.paths.state_file, init.paths.state_file);
    assert_eq!(store.pragma_value("foreign_keys").unwrap(), "1");
    assert!(init.paths.logs_dir.is_dir());
    assert!(init.paths.artifacts_dir.is_dir());

    let not_git = create_fixture(temp.path(), FixtureKind::NotGitRepo);
    let err = init_repo(Some(&not_git.path)).expect_err("not-git fixture should fail init");
    assert!(err.to_string().contains("not a git repository"));
}

#[test]
fn production_default_service_run_uses_external_worktree_and_persists_manifest() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_fixture(temp.path(), FixtureKind::RustSuccess);
    let init = init_repo(Some(&fixture.path)).expect("init repo");
    let loaded = load_config(Some(&fixture.path)).expect("load config");
    let configured_worktree_root = PathBuf::from(&loaded.config.workspace.worktree_root);
    let store = Arc::new(SqliteTaskStore::open(&init.paths.state_file).expect("open state"));
    let workspace = Arc::new(FixedRepoWorkspace::new(
        &fixture.path,
        &loaded.paths.worktree_root,
    ));
    let workspace_manager: Arc<dyn WorkspaceManager> = workspace.clone();
    let command_runner: Arc<dyn CommandRunner> = workspace.clone();
    let local = FakeModelProvider::new("fake-ollama");
    local.push_text(secret_success_patch());
    let service = DefaultHarnessService::from_parts(
        loaded.config,
        store.clone(),
        workspace_manager,
        command_runner,
        Arc::new(local.clone()),
        Arc::new(FakeModelProvider::new("fake-openai")),
    );
    let rust_tool_dir = Path::new(env!("CARGO")).parent().unwrap().to_string_lossy();
    let validation = format!("PATH={rust_tool_dir}:/usr/bin:/bin {} test", env!("CARGO"));
    let goal = format!("Make the intentionally failing test pass. Evidence includes {SECRET}");

    let created = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "task",
            "create",
            "--title",
            "Fix add",
            "--goal",
            goal.as_str(),
            "--validation",
            validation.as_str(),
        ],
    );
    assert_exit(&created.exit, CommandStatus::Complete, 0);
    let task_id = TaskId::parse(json_str(&created.stdout, "/data/task_id")).unwrap();

    let ran = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "task",
            "run",
            task_id.as_str(),
            "--max-attempts",
            "1",
            "--model",
            "fake-local-model",
        ],
    );
    if ran.exit.status != CommandStatus::Complete {
        let latest = store.latest_run_for_task(&task_id).unwrap();
        let attempts = latest
            .as_ref()
            .map(|run| store.list_attempts(&run.id).unwrap())
            .unwrap_or_default();
        let validation_log = attempts
            .iter()
            .filter_map(|attempt| attempt.validation_log_path.as_deref())
            .filter_map(|path| fs::read_to_string(path).ok())
            .collect::<Vec<_>>()
            .join("\n--- validation log ---\n");
        panic!(
            "production run failed: {:?}\nstdout:\n{}\nstderr:\n{}\nrun:{latest:?}\nattempts:{attempts:?}\nvalidation_log:\n{validation_log}",
            ran.exit, ran.stdout, ran.stderr
        );
    }
    assert_exit(&ran.exit, CommandStatus::Complete, 0);

    let task = store.get_task(&task_id).unwrap();
    let worktree = PathBuf::from(task.worktree_path.as_deref().unwrap());
    assert_ne!(worktree, fixture.path);
    let canonical_worktree = worktree.canonicalize().unwrap();
    let canonical_worktree_root = configured_worktree_root.canonicalize().unwrap();
    let canonical_fixture = fixture.path.canonicalize().unwrap();
    assert!(canonical_worktree.starts_with(&canonical_worktree_root));
    assert!(!canonical_worktree.starts_with(&canonical_fixture));
    assert_clean_git_repo(&fixture.path);

    let run = store.latest_run_for_task(&task_id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Complete);
    assert_eq!(run.base_commit.len(), 40);
    let artifacts = store.list_artifacts_for_run(&run.id).unwrap();
    for artifact in &artifacts {
        let path = PathBuf::from(&artifact.path);
        assert!(path.exists(), "{}", path.display());
        assert_eq!(artifact.sha256, sha256_file(&path), "{}", path.display());
        assert_eq!(artifact.byte_len, fs::metadata(&path).unwrap().len());
    }

    let manifest_artifact = artifacts
        .iter()
        .find(|artifact| artifact.kind == "manifest")
        .expect("manifest artifact");
    let manifest_text = fs::read_to_string(&manifest_artifact.path).unwrap();
    let manifest: ArtifactManifest = serde_json::from_str(&manifest_text)
        .unwrap_or_else(|error| panic!("manifest parse failed: {error}\n{manifest_text}"));
    assert_eq!(manifest.provider.as_deref(), Some("fake-ollama"));
    assert_eq!(manifest.model.as_deref(), Some("fake-local-model"));
    assert_eq!(
        manifest.base_commit.as_deref(),
        Some(run.base_commit.as_str())
    );
    assert_eq!(
        manifest.pre_attempt_head.as_deref(),
        Some(run.base_commit.as_str())
    );
    assert_eq!(
        manifest.validation_command.as_deref(),
        Some(validation.as_str())
    );
    assert!(
        manifest
            .post_attempt_head
            .as_deref()
            .is_some_and(|head| head.len() == 40)
    );
    assert!(
        manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "prompt")
    );
    assert!(
        manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "patch")
    );
    for record in &manifest.artifacts {
        assert_eq!(record.sha256, sha256_file(Path::new(&record.path)));
        assert_eq!(record.byte_len, fs::metadata(&record.path).unwrap().len());
    }

    let requests = local.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model, "fake-local-model");
    assert!(
        requests[0]
            .input
            .contains("Make the intentionally failing test pass")
    );
    assert_no_secret_in_persisted_outputs(
        store.as_ref(),
        &init.paths.state_file,
        &init.paths.artifacts_dir,
        &[&created.stdout, &created.stderr, &ran.stdout, &ran.stderr],
    );
    assert_no_secret_in_provider_requests(&local);
}

#[test]
fn production_default_service_ticket_resolve_and_resume_redact_secret_data() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_fixture(temp.path(), FixtureKind::RustResumeAfterTicket);
    let init = init_repo(Some(&fixture.path)).expect("init repo");
    let loaded = load_config(Some(&fixture.path)).expect("load config");
    let store = Arc::new(SqliteTaskStore::open(&init.paths.state_file).expect("open state"));
    let workspace = Arc::new(FixedRepoWorkspace::new(
        &fixture.path,
        &loaded.paths.worktree_root,
    ));
    let workspace_manager: Arc<dyn WorkspaceManager> = workspace.clone();
    let command_runner: Arc<dyn CommandRunner> = workspace.clone();
    let local = FakeModelProvider::new("fake-ollama");
    local.push_text(
        "STUCK\nreason: need product decision before normalizing input\nquestion: Should normalize trim and lowercase ASCII-compatible input?",
    );
    local.push_text(resume_patch());
    let openai = FakeModelProvider::new("fake-openai");
    openai.push_text_with_id(
        "resp_fake_002",
        format!(
            "Decision: trim surrounding whitespace and lowercase the result.\nDo not leak {SECRET}."
        ),
    );
    let service = DefaultHarnessService::from_parts(
        loaded.config,
        store.clone(),
        workspace_manager,
        command_runner,
        Arc::new(local.clone()),
        Arc::new(openai.clone()),
    );
    let rust_tool_dir = Path::new(env!("CARGO")).parent().unwrap().to_string_lossy();
    let validation = format!("PATH={rust_tool_dir}:/usr/bin:/bin {} test", env!("CARGO"));
    let goal = format!("Make normalize pass. Evidence includes {SECRET}");

    let created = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "task",
            "create",
            "--title",
            "Resolve normalization",
            "--goal",
            goal.as_str(),
            "--validation",
            validation.as_str(),
        ],
    );
    assert_exit(&created.exit, CommandStatus::Complete, 0);
    let task_id = TaskId::parse(json_str(&created.stdout, "/data/task_id")).unwrap();

    let stuck = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "task",
            "run",
            task_id.as_str(),
            "--max-attempts",
            "1",
            "--model",
            "fake-local-model",
        ],
    );
    assert_exit(&stuck.exit, CommandStatus::Stuck, 10);
    let ticket_id = TicketId::parse(json_str(&stuck.stdout, "/data/ticket_id")).unwrap();

    let resolved = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "ticket",
            "resolve",
            ticket_id.as_str(),
            "--model",
            "fake-openai-model",
        ],
    );
    assert_exit(&resolved.exit, CommandStatus::Complete, 0);
    let resolution_path = store
        .list_ticket_resolutions(&ticket_id)
        .unwrap()
        .into_iter()
        .next()
        .expect("ticket resolution")
        .resolution_path;
    let resolution_text = fs::read_to_string(&resolution_path).unwrap();
    assert!(!resolution_text.contains("sk-testsecret"));
    assert!(resolution_text.contains("[REDACTED"));

    let resumed = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "resume",
            task_id.as_str(),
            "--ticket",
            ticket_id.as_str(),
            "--max-attempts",
            "1",
            "--model",
            "fake-local-model",
        ],
    );
    assert_exit(&resumed.exit, CommandStatus::Complete, 0);
    assert_eq!(
        store.get_task(&task_id).unwrap().status,
        TaskStatus::Complete
    );
    assert!(
        store
            .list_ticket_resolutions(&ticket_id)
            .unwrap()
            .iter()
            .all(|resolution| resolution.consumed_at.is_some())
    );

    assert_no_secret_in_persisted_outputs(
        store.as_ref(),
        &init.paths.state_file,
        &init.paths.artifacts_dir,
        &[
            &created.stdout,
            &created.stderr,
            &stuck.stdout,
            &stuck.stderr,
            &resolved.stdout,
            &resolved.stderr,
            &resumed.stdout,
            &resumed.stderr,
        ],
    );
    assert_eq!(local.requests().len(), 2);
    assert_eq!(openai.requests().len(), 1);
    assert_no_secret_in_provider_requests(&local);
    assert_no_secret_in_provider_requests(&openai);
}

#[test]
fn task_create_and_run_success_persist_state_artifacts_manifest_and_provider_request() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_fixture(temp.path(), FixtureKind::RustSuccess);
    let local = FakeModelProvider::new("fake-ollama");
    local.push_text(success_patch());
    let service = E2eService::new(
        &fixture.path,
        local.clone(),
        FakeModelProvider::new("fake-openai"),
    );

    let created = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "task",
            "create",
            "--title",
            "Fix add",
            "--goal",
            "Make the intentionally failing test pass",
            "--validation",
            "cargo test",
        ],
    );
    assert_exit(&created.exit, CommandStatus::Complete, 0);
    let task_id = json_str(&created.stdout, "/data/task_id");

    let ran = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "task",
            "run",
            task_id.as_str(),
            "--max-attempts",
            "1",
            "--model",
            "fake-local-model",
        ],
    );
    assert_exit(&ran.exit, CommandStatus::Complete, 0);
    assert!(ran.stderr.is_empty());

    let task = service
        .store
        .get_task(&TaskId::parse(task_id.clone()).unwrap())
        .unwrap();
    assert_eq!(task.status, TaskStatus::Complete);
    assert_eq!(
        task.worktree_path.as_deref(),
        Some(fixture.path.to_str().unwrap())
    );
    assert_eq!(
        service
            .store
            .list_validation_commands(&task.id)
            .unwrap()
            .into_iter()
            .map(|command| command.command)
            .collect::<Vec<_>>(),
        vec!["cargo test"]
    );

    let run_id = json_str(&ran.stdout, "/data/run_id");
    let run = service
        .store
        .get_run(&RunId::parse(run_id).unwrap())
        .unwrap();
    assert_eq!(run.status, RunStatus::Complete);
    assert!(PathBuf::from(run.final_diff_path.unwrap()).exists());

    let artifacts = service.store.list_artifacts_for_run(&run.id).unwrap();
    assert!(artifacts.iter().any(|artifact| artifact.kind == "prompt"));
    assert!(
        artifacts
            .iter()
            .any(|artifact| artifact.kind == "provider_response")
    );
    assert!(artifacts.iter().any(|artifact| artifact.kind == "patch"));
    assert!(
        artifacts
            .iter()
            .any(|artifact| artifact.kind == "validation_log")
    );
    for artifact in &artifacts {
        let path = PathBuf::from(&artifact.path);
        assert!(path.exists(), "{}", artifact.path);
        assert_eq!(artifact.sha256, sha256_file(&path), "{}", artifact.path);
        assert_eq!(artifact.byte_len, fs::metadata(&path).unwrap().len());
    }

    let manifest = build_artifact_manifest(
        &artifacts,
        Some("fake-ollama"),
        Some("fake-local-model"),
        Some("fake"),
        json!({ "temperature": 0.0 }),
    );
    assert_eq!(manifest.artifacts.len(), artifacts.len());
    assert!(
        manifest
            .artifacts
            .iter()
            .all(|record| record.sha256.len() == 64)
    );

    let requests = local.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model, "fake-local-model");
    assert!(
        requests[0]
            .input
            .contains("Make the intentionally failing test pass")
    );
}

#[test]
fn validation_failure_then_stuck_returns_ticket_and_records_failed_attempt() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_fixture(temp.path(), FixtureKind::RustValidationFailsThenStuck);
    let local = FakeModelProvider::new("fake-ollama");
    local.push_text(still_failing_patch());
    local.push_text("STUCK\nreason: validation still fails after the attempted fix\nquestion: Which even-number behavior should be preserved?");
    let service = E2eService::new(
        &fixture.path,
        local.clone(),
        FakeModelProvider::new("fake-openai"),
    );
    let task = service
        .create_task(
            "Fix even detection".to_string(),
            "Make even validation pass".to_string(),
            vec!["cargo test".to_string()],
        )
        .unwrap();

    let ran = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "task",
            "run",
            task.id.as_str(),
            "--max-attempts",
            "2",
        ],
    );
    assert_exit(&ran.exit, CommandStatus::Stuck, 10);
    let ticket_id = json_str(&ran.stdout, "/data/ticket_id");

    let task = service.store.get_task(&task.id).unwrap();
    assert_eq!(task.status, TaskStatus::Stuck);
    let ticket = service
        .store
        .get_ticket(&TicketId::parse(ticket_id).unwrap())
        .unwrap();
    assert_eq!(ticket.status, TicketStatus::Open);
    assert_eq!(ticket.blocked_on, "validation");
    assert!(ticket.question.contains("Which even-number behavior"));

    let attempts = service
        .store
        .list_attempts(&service.store.get_run(&ticket.run_id).unwrap().id)
        .unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].status, AttemptStatus::ValidationFailed);
    assert_eq!(attempts[0].validation_exit_code, Some(101));
    assert_eq!(attempts[1].status, AttemptStatus::Complete);
    assert_eq!(local.requests().len(), 2);
}

#[test]
fn ticket_resolve_redacts_provider_output_and_resume_consumes_resolution() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_fixture(temp.path(), FixtureKind::RustResumeAfterTicket);
    let local = FakeModelProvider::new("fake-ollama");
    local.push_text(
        "STUCK\nreason: need product decision before normalizing input\nquestion: Should normalize trim and lowercase ASCII-compatible input?",
    );
    local.push_text(resume_patch());
    let openai = FakeModelProvider::new("fake-openai");
    openai.push_text_with_id(
        "resp_fake_001",
        format!(
            "Decision: trim surrounding whitespace and lowercase the result.\nDo not leak {SECRET}."
        ),
    );
    let service = E2eService::new(&fixture.path, local.clone(), openai.clone());

    let created = service
        .create_task(
            "Resolve normalization".to_string(),
            format!("Make normalize pass. Evidence includes {SECRET}"),
            vec!["cargo test".to_string()],
        )
        .unwrap();
    let stuck = service
        .run_task(
            &created.id,
            TaskRunOptions {
                runtime: RuntimeOptions {
                    output: OutputMode::Json,
                    ..RuntimeOptions::default()
                },
                max_attempts: Some(1),
                model: Some("fake-local-model".to_string()),
            },
        )
        .unwrap();
    assert_eq!(stuck.exit.code(), 10);
    let ticket_id = TicketId::parse(stuck.data["ticket_id"].as_str().unwrap()).unwrap();

    let resolved = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "ticket",
            "resolve",
            ticket_id.as_str(),
            "--model",
            "fake-openai-model",
        ],
    );
    assert_exit(&resolved.exit, CommandStatus::Complete, 0);
    let resolution_path = PathBuf::from(json_str(&resolved.stdout, "/data/resolution_path"));
    let resolution_text = fs::read_to_string(&resolution_path).unwrap();
    assert!(!resolution_text.contains("sk-testsecret"));
    assert!(resolution_text.contains("[REDACTED"));

    let resolutions = service.store.list_ticket_resolutions(&ticket_id).unwrap();
    assert_eq!(resolutions.len(), 1);
    assert_eq!(resolutions[0].response_id.as_deref(), Some("resp_fake_001"));
    assert!(resolutions[0].consumed_at.is_none());
    let evidence = ticket_resolution_text(&resolutions[0], &resolution_text);
    assert!(evidence.body.contains("trim surrounding whitespace"));

    let resumed = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "resume",
            created.id.as_str(),
            "--ticket",
            ticket_id.as_str(),
            "--max-attempts",
            "1",
            "--model",
            "fake-local-model",
        ],
    );
    assert_exit(&resumed.exit, CommandStatus::Complete, 0);
    assert_eq!(
        service.store.get_task(&created.id).unwrap().status,
        TaskStatus::Complete
    );
    assert!(
        service
            .store
            .list_ticket_resolutions(&ticket_id)
            .unwrap()
            .iter()
            .all(|resolution| resolution.consumed_at.is_some())
    );

    assert_no_secret_in_service_outputs(
        &service,
        &[&resolved.stdout, &resolved.stderr, &resumed.stdout],
    );
    assert_eq!(openai.requests().len(), 1);
    assert!(!openai.requests()[0].input.contains("sk-testsecret"));
    assert!(openai.requests()[0].input.contains("[REDACTED"));
    assert_eq!(local.requests().len(), 2);
    assert_no_secret_in_provider_requests(&local);
    assert_no_secret_in_provider_requests(&openai);
}

#[test]
fn patch_safety_rejection_blocks_malicious_provider_diff() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = create_fixture(temp.path(), FixtureKind::RustSuccess);
    let local = FakeModelProvider::new("fake-ollama");
    local.push_text(
        "```diff\ndiff --git a/../outside.txt b/../outside.txt\n--- a/../outside.txt\n+++ b/../outside.txt\n@@ -1 +1 @@\n-old\n+new\n```",
    );
    let service = E2eService::new(&fixture.path, local, FakeModelProvider::new("fake-openai"));
    let task = service
        .create_task(
            "Reject unsafe patch".to_string(),
            "Provider must not write outside the repo".to_string(),
            vec!["cargo test".to_string()],
        )
        .unwrap();

    let ran = dispatch_json(
        &service,
        [
            "harness",
            "--output",
            "json",
            "task",
            "run",
            task.id.as_str(),
            "--max-attempts",
            "1",
        ],
    );

    assert_exit(&ran.exit, CommandStatus::SecurityBlocked, 30);
    assert!(ran.stderr.is_empty());
    let task = service.store.get_task(&task.id).unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
    let run = service
        .store
        .latest_run_for_task(&task.id)
        .unwrap()
        .unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    let last_error = run.last_error.unwrap();
    assert!(
        last_error.contains("escapes worktree") || last_error.contains("path traversal"),
        "{last_error}"
    );

    let direct = validate_patch_safety(
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-a\n+b\n",
        &PatchValidationConfig {
            worktree_path: fixture.path.to_string_lossy().into_owned(),
            max_patch_bytes: 10,
            max_files_changed: 20,
        },
    );
    assert!(direct.unwrap_err().to_string().contains("byte limit"));

    fs::write(fixture.path.join("keep.txt"), "old\n").unwrap();
    #[cfg(unix)]
    {
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, "old\n").unwrap();
        std::os::unix::fs::symlink(&outside, fixture.path.join("escape-link.txt")).unwrap();
    }
    for (name, diff, expected) in [
        (
            "absolute path",
            "diff --git a//tmp/x b//tmp/x\n--- a//tmp/x\n+++ b//tmp/x\n@@ -1 +1 @@\n-a\n+b\n",
            "absolute",
        ),
        (
            "git hook",
            "diff --git a/.git/hooks/pre-commit b/.git/hooks/pre-commit\n--- a/.git/hooks/pre-commit\n+++ b/.git/hooks/pre-commit\n@@ -1 +1 @@\n-a\n+b\n",
            ".git",
        ),
        (
            "delete",
            "diff --git a/keep.txt b/keep.txt\ndeleted file mode 100644\n--- a/keep.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n",
            "deletes",
        ),
        (
            "rename",
            "diff --git a/keep.txt b/moved.txt\nsimilarity index 100%\nrename from keep.txt\nrename to moved.txt\n",
            "renames",
        ),
        (
            "binary",
            "diff --git a/keep.txt b/keep.txt\nBinary files a/keep.txt and b/keep.txt differ\n",
            "binary",
        ),
    ] {
        let error = validate_patch_safety(diff, &patch_config(&fixture.path))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(expected),
            "{name} expected {expected:?}, got {error}"
        );
    }

    #[cfg(unix)]
    {
        let error = validate_patch_safety(
            "diff --git a/escape-link.txt b/escape-link.txt\n--- a/escape-link.txt\n+++ b/escape-link.txt\n@@ -1 +1 @@\n-old\n+new\n",
            &patch_config(&fixture.path),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("escapes worktree") || error.contains("normal file"));
    }
}

struct DispatchOutput {
    exit: CommandExit,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
struct CliOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

struct FixedRepoWorkspace {
    repo: PathBuf,
    inner: GitWorkspaceManager,
}

impl FixedRepoWorkspace {
    fn new(repo: &Path, worktree_root: &Path) -> Self {
        Self {
            repo: repo.to_path_buf(),
            inner: GitWorkspaceManager::new(repo, worktree_root).expect("workspace manager"),
        }
    }
}

impl WorkspaceManager for FixedRepoWorkspace {
    fn discover_repo_root(&self, repo: Option<&str>) -> HarnessResult<String> {
        match repo {
            Some(repo) => self.inner.discover_repo_root(Some(repo)),
            None => Ok(self.repo.to_string_lossy().into_owned()),
        }
    }

    fn source_is_dirty(&self, repo_root: &str) -> HarnessResult<bool> {
        self.inner.source_is_dirty(repo_root)
    }

    fn resolve_base_commit(
        &self,
        repo_root: &str,
        base_ref: Option<&str>,
    ) -> HarnessResult<String> {
        self.inner.resolve_base_commit(repo_root, base_ref)
    }

    fn ensure_task_worktree(&self, request: WorktreeRequest) -> HarnessResult<WorktreeInfo> {
        self.inner.ensure_task_worktree(request)
    }

    fn verify_recorded_worktree(
        &self,
        repo_root: &str,
        recorded: &RecordedWorktree,
    ) -> HarnessResult<WorktreeInfo> {
        self.inner.verify_recorded_worktree(repo_root, recorded)
    }

    fn capture_diff(&self, worktree_path: &str, run_id: &RunId) -> HarnessResult<String> {
        self.inner.capture_diff(worktree_path, run_id)
    }

    fn check_patch(&self, patch: PatchCheck) -> HarnessResult<PatchCheckResult> {
        self.inner.check_patch(patch)
    }

    fn apply_patch(&self, patch: PatchCheck) -> HarnessResult<PatchApplyResult> {
        self.inner.apply_patch(patch)
    }

    fn commit_all(&self, worktree_path: &str, message: &str) -> HarnessResult<String> {
        self.inner.commit_all(worktree_path, message)
    }

    fn fast_forward_repo(&self, repo_root: &str, branch: &str) -> HarnessResult<String> {
        self.inner.fast_forward_repo(repo_root, branch)
    }

    fn cleanup_task_worktree(&self, task_id: &TaskId, force: bool) -> HarnessResult<()> {
        self.inner.cleanup_task_worktree(task_id, force)
    }
}

impl CommandRunner for FixedRepoWorkspace {
    fn run_validation(&self, spec: CommandSpec) -> HarnessResult<CommandOutput> {
        self.inner.run_validation(spec)
    }

    fn run_shell_escape(&self, spec: CommandSpec) -> HarnessResult<CommandOutput> {
        self.inner.run_shell_escape(spec)
    }
}

fn run_harness_cli<'a>(repo: &Path, args: impl IntoIterator<Item = &'a str>) -> CliOutput {
    let output = Command::new(env!("CARGO_BIN_EXE_harness"))
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run harness binary");
    CliOutput {
        status: output.status,
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

fn cargo_validation_command() -> String {
    let rust_tool_dir = Path::new(env!("CARGO")).parent().unwrap().to_string_lossy();
    format!("PATH={rust_tool_dir}:/usr/bin:/bin {} test", env!("CARGO"))
}

fn assert_binary_json_contract(
    output: &binary::BinaryOutput,
    expected_status: &str,
    expected_exit_code: i32,
) -> Value {
    let stdout_lines = output.stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        stdout_lines.len(),
        1,
        "stdout must be exactly one final JSON object\nstdout:\n{}\nstderr:\n{}",
        output.stdout,
        output.stderr
    );
    let value: Value = serde_json::from_str(stdout_lines[0]).unwrap_or_else(|error| {
        panic!(
            "stdout final line is not JSON: {error}\nstdout:\n{}\nstderr:\n{}",
            output.stdout, output.stderr
        )
    });
    assert_eq!(value["status"], expected_status, "{value:#?}");
    assert_eq!(value["exit_code"], expected_exit_code, "{value:#?}");
    assert_eq!(
        output.status.code(),
        Some(expected_exit_code),
        "process exit must match final exit_code\nstdout:\n{}\nstderr:\n{}",
        output.stdout,
        output.stderr
    );
    value
}

fn assert_supervise_ndjson_phases(stderr: &str, expected_phases: &[&str]) -> Vec<Value> {
    assert!(
        !stderr.trim().is_empty(),
        "expected progress NDJSON on stderr"
    );
    assert!(
        !stderr.contains("info:") && !stderr.contains("warn:") && !stderr.contains("error:"),
        "{stderr}"
    );
    let events = stderr
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap_or_else(|_| panic!("{line}")))
        .collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .map(|event| event["phase"].as_str().unwrap_or("<missing>"))
            .collect::<Vec<_>>(),
        expected_phases,
        "{events:#?}"
    );
    for event in &events {
        assert_eq!(event["event"], "supervise.phase", "{event:#?}");
        assert!(event["task_id"].as_str().is_some(), "{event:#?}");
        assert!(
            event["message"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
        assert_eq!(event["status"], event["phase"], "{event:#?}");
        assert_eq!(event["elapsed_ms"], 0, "{event:#?}");
    }
    events
}

fn assert_objective_ndjson_events(stderr: &str) -> Vec<Value> {
    assert!(
        !stderr.trim().is_empty(),
        "expected objective NDJSON on stderr"
    );
    assert!(
        !stderr.contains("info:") && !stderr.contains("warn:") && !stderr.contains("error:"),
        "{stderr}"
    );
    let events = stderr
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap_or_else(|_| panic!("{line}")))
        .collect::<Vec<_>>();
    for event in &events {
        let event_name = event["event"].as_str().unwrap_or("<missing>");
        assert!(
            event_name.starts_with("objective."),
            "expected objective event, got {event:#?}"
        );
        assert_eq!(event["schema_version"], 1, "{event:#?}");
        assert!(event["objective_id"].as_str().is_some(), "{event:#?}");
        assert!(event["phase"].as_str().is_some(), "{event:#?}");
        assert!(event["status"].as_str().is_some(), "{event:#?}");
        assert!(
            event["message"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "{event:#?}"
        );
    }
    events
}

fn assert_nonzero_next_commands(value: &Value) {
    assert_ne!(value["exit_code"], 0, "{value:#?}");
    assert!(
        value["data"]["next_commands"]
            .as_array()
            .is_some_and(|commands| !commands.is_empty()),
        "{value:#?}"
    );
}

fn json_string_array(value: &Value, pointer: &str) -> Vec<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("missing JSON array at {pointer} in {value}"))
        .iter()
        .map(|item| {
            item.as_str()
                .unwrap_or_else(|| panic!("non-string item in {pointer}: {value}"))
                .to_string()
        })
        .collect()
}

fn provider_requests_for_path(
    requests: &[harness::providers::FakeHttpRequest],
    path: &str,
) -> Vec<Value> {
    requests
        .iter()
        .filter(|request| request.method == "POST" && request.path == path)
        .map(|request| request.json_body().unwrap())
        .collect()
}

fn assert_run_artifacts_have_manifest(store: &dyn TaskStore, run_id: &RunId) {
    let artifacts = store.list_artifacts_for_run(run_id).unwrap();
    assert!(!artifacts.is_empty(), "run {run_id} has no artifacts");
    assert!(
        artifacts.iter().any(|artifact| artifact.kind == "manifest"),
        "{artifacts:#?}"
    );
    for artifact in &artifacts {
        let path = PathBuf::from(&artifact.path);
        assert!(path.exists(), "{}", path.display());
        assert_eq!(artifact.sha256, sha256_file(&path), "{}", path.display());
        assert_eq!(artifact.byte_len, fs::metadata(&path).unwrap().len());
        if artifact.kind == "manifest" {
            let text = fs::read_to_string(&path).unwrap();
            let manifest: ArtifactManifest = serde_json::from_str(&text)
                .unwrap_or_else(|error| panic!("manifest parse failed: {error}\n{text}"));
            assert!(!manifest.prompt_contract_version.is_empty());
            for record in &manifest.artifacts {
                assert!(Path::new(&record.path).exists(), "{}", record.path);
                assert_eq!(record.sha256, sha256_file(Path::new(&record.path)));
                assert_eq!(record.byte_len, fs::metadata(&record.path).unwrap().len());
            }
        }
    }
}

fn assert_no_secret_in_binary_surfaces(
    paths: &ConfigPaths,
    outputs: &[&str],
    provider_requests: &[&[harness::providers::FakeHttpRequest]],
) {
    let store = SqliteTaskStore::open(&paths.state_file).expect("open sqlite state");
    assert_no_secret_in_persisted_outputs(&store, &paths.state_file, &paths.artifacts_dir, outputs);
    for requests in provider_requests {
        for request in *requests {
            assert!(
                !request.body_string().contains("sk-testsecret"),
                "{request:?}"
            );
            assert!(
                !format!("{:?}", request.headers).contains("sk-testsecret"),
                "{request:?}"
            );
        }
    }
}

fn dispatch_json<'a>(
    service: &dyn HarnessService,
    args: impl IntoIterator<Item = &'a str>,
) -> DispatchOutput {
    let runtime = CommandRuntime::new(service);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut sink = JsonSink::new(&mut stdout, &mut stderr, false);
    let exit = runtime.dispatch(args, &mut sink);

    DispatchOutput {
        exit,
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
    }
}

fn assert_clean_git_repo(repo: &Path) {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1"])
        .current_dir(repo)
        .output()
        .expect("run git status");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).unwrap();
    sha256_bytes(&bytes)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn assert_exit(exit: &CommandExit, status: CommandStatus, code: u8) {
    assert_eq!(exit.status, status, "{exit:?}");
    assert_eq!(exit.code(), code, "{exit:?}");
}

fn patch_config(worktree: &Path) -> PatchValidationConfig {
    PatchValidationConfig {
        worktree_path: worktree.to_string_lossy().into_owned(),
        max_patch_bytes: 131_072,
        max_files_changed: 20,
    }
}

fn json_str(output: &str, pointer: &str) -> String {
    let value: Value = serde_json::from_str(output.trim()).expect(output);
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing JSON string at {pointer} in {value}"))
        .to_string()
}

fn block_on_provider<T>(future: ProviderFuture<'_, T>) -> harness::providers::ProviderResult<T> {
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    let mut future = future;
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => return value,
            std::task::Poll::Pending => std::thread::yield_now(),
        }
    }
}

struct E2eService {
    repo: PathBuf,
    paths: ConfigPaths,
    store: SqliteTaskStore,
    local: FakeModelProvider,
    openai: FakeModelProvider,
    ids: Mutex<IdAllocator>,
    redactor: DefaultRedactor,
}

impl E2eService {
    fn new(repo: &Path, local: FakeModelProvider, openai: FakeModelProvider) -> Self {
        let init = init_repo(Some(repo)).expect("init fixture repo");
        let store = SqliteTaskStore::open(&init.paths.state_file).expect("open state");
        Self {
            repo: repo.to_path_buf(),
            paths: init.paths,
            store,
            local,
            openai,
            ids: Mutex::new(IdAllocator::default()),
            redactor: DefaultRedactor::new(),
        }
    }

    fn next_task_id(&self) -> TaskId {
        self.ids.lock().unwrap().task()
    }

    fn next_run_id(&self) -> RunId {
        self.ids.lock().unwrap().run()
    }

    fn next_attempt_id(&self) -> AttemptId {
        self.ids.lock().unwrap().attempt()
    }

    fn next_ticket_id(&self) -> TicketId {
        self.ids.lock().unwrap().ticket()
    }

    fn next_resolution_id(&self) -> TicketResolutionId {
        self.ids.lock().unwrap().resolution()
    }

    fn next_artifact_id(&self) -> ArtifactId {
        self.ids.lock().unwrap().artifact()
    }

    fn base_commit(&self) -> HarnessResult<String> {
        command_stdout(
            Command::new("git")
                .arg("-C")
                .arg(&self.repo)
                .args(["rev-parse", "HEAD"]),
        )
    }

    fn run_attempts(
        &self,
        task_id: &TaskId,
        parent_run_id: Option<RunId>,
        max_attempts: u32,
        model: String,
        resolution: Option<TicketResolution>,
    ) -> HarnessResult<CommandResult> {
        self.store.acquire_task_lease(task_id, OWNER)?;
        let mut task = self.store.get_task(task_id)?;
        let from_status = task.status;
        self.store
            .transition_task(task_id, from_status, TaskStatus::Running, OWNER)?;

        task = self.store.get_task(task_id)?;
        task.worktree_path = Some(self.repo.to_string_lossy().into_owned());
        task.base_ref = Some("HEAD".to_string());
        task.base_commit = Some(self.base_commit()?);
        task.updated_at = now();
        self.store.update_task(task.clone(), OWNER)?;

        let run_id = self.next_run_id();
        let run = Run {
            id: run_id.clone(),
            task_id: task_id.clone(),
            parent_run_id,
            status: RunStatus::Running,
            repo_root: self.repo.to_string_lossy().into_owned(),
            base_ref: Some("HEAD".to_string()),
            base_commit: task.base_commit.clone().unwrap(),
            dirty_state_summary: None,
            current_phase: Some("provider".to_string()),
            escalation_cycle: u32::from(resolution.is_some()),
            started_at: now(),
            finished_at: None,
            final_diff_path: None,
            last_error: None,
        };
        self.store.insert_run(run.clone(), OWNER)?;

        if let Some(resolution) = &resolution {
            self.store
                .mark_ticket_resolution_consumed(&resolution.id, &now(), OWNER)?;
        }

        let validation = self.store.list_validation_commands(task_id)?;
        let validation_command = validation
            .first()
            .ok_or_else(|| HarnessError::Usage("task has no validation command".to_string()))?
            .command
            .clone();

        for attempt_number in 1..=max_attempts {
            let prompt = self.prompt(&task, resolution.as_ref());
            let prompt_path = self.write_run_artifact(task_id, &run_id, "prompt", &prompt)?;
            let response = block_on_provider(self.local.complete(ModelRequest {
                model: model.clone(),
                system: Some("Return exactly one diff fence or STUCK block.".to_string()),
                input: prompt.clone(),
                temperature: Some(0.0),
                max_output_tokens: Some(4096),
                metadata: BTreeMap::from([
                    ("task_id".to_string(), task_id.as_str().to_string()),
                    ("run_id".to_string(), run_id.as_str().to_string()),
                ]),
            }))
            .map_err(provider_error)?;

            let response_text = self.redactor.redact(&response.text).text;
            let response_path =
                self.write_run_artifact(task_id, &run_id, "provider_response", &response_text)?;
            match parse_ollama_response(&response.text)? {
                OllamaResponse::Stuck(stuck) => {
                    let attempt = self.insert_attempt(
                        &run_id,
                        attempt_number,
                        &model,
                        AttemptStatus::Complete,
                        Some(prompt_path),
                        Some(response_path.clone()),
                        None,
                        None,
                        None,
                        None,
                        None,
                    )?;
                    let ticket =
                        self.insert_ticket(task_id, &run_id, &stuck.reason, &stuck.question)?;
                    self.store.insert_artifact(
                        artifact(
                            self.next_artifact_id(),
                            task_id.clone(),
                            Some(run_id.clone()),
                            Some(attempt.id),
                            Some(ticket.id.clone()),
                            "stuck_response",
                            response_path,
                        ),
                        OWNER,
                    )?;
                    self.finish_run_stuck(&run_id, &ticket.reason)?;
                    self.store.transition_task(
                        task_id,
                        TaskStatus::Running,
                        TaskStatus::Stuck,
                        OWNER,
                    )?;
                    self.store.release_task_lease(task_id, OWNER)?;
                    return Ok(CommandResult::with_data(
                        CommandExit::stuck(format!("task {task_id} is stuck")),
                        json!({
                            "task_id": task_id.as_str(),
                            "run_id": run_id.as_str(),
                            "ticket_id": ticket.id.as_str(),
                        }),
                    ));
                }
                OllamaResponse::Patch(parsed) => {
                    let patch_path =
                        self.write_run_artifact(task_id, &run_id, "patch", &parsed.diff)?;
                    if let Err(error) = validate_patch_safety(
                        &parsed.diff,
                        &PatchValidationConfig {
                            worktree_path: self.repo.to_string_lossy().into_owned(),
                            max_patch_bytes: 131_072,
                            max_files_changed: 20,
                        },
                    ) {
                        self.insert_attempt(
                            &run_id,
                            attempt_number,
                            &model,
                            AttemptStatus::PatchRejected,
                            Some(prompt_path),
                            Some(response_path),
                            Some(patch_path),
                            None,
                            None,
                            Some(error.to_string()),
                            Some(error.to_string()),
                        )?;
                        self.fail_run_and_task(task_id, &run_id, &error.to_string())?;
                        self.store.release_task_lease(task_id, OWNER)?;
                        return Ok(CommandResult::with_data(
                            CommandExit::security_blocked(error.to_string()),
                            json!({ "task_id": task_id.as_str(), "run_id": run_id.as_str() }),
                        ));
                    }

                    apply_patch(&self.repo, &parsed.diff)?;
                    let validation_output = run_validation(&self.repo, &validation_command)?;
                    let validation_path = self.write_run_artifact(
                        task_id,
                        &run_id,
                        "validation_log",
                        &validation_output.text,
                    )?;
                    let validation_status = if validation_output.code == 0 {
                        AttemptStatus::Complete
                    } else {
                        AttemptStatus::ValidationFailed
                    };
                    self.insert_attempt(
                        &run_id,
                        attempt_number,
                        &model,
                        validation_status,
                        Some(prompt_path),
                        Some(response_path),
                        Some(patch_path.clone()),
                        Some(validation_path),
                        Some(validation_output.code),
                        (validation_output.code != 0).then(|| "validation failed".to_string()),
                        None,
                    )?;

                    if validation_output.code == 0 {
                        self.finish_run_complete(&run_id, &patch_path)?;
                        self.store.transition_task(
                            task_id,
                            TaskStatus::Running,
                            TaskStatus::Complete,
                            OWNER,
                        )?;
                        self.store.release_task_lease(task_id, OWNER)?;
                        return Ok(CommandResult::with_data(
                            CommandExit::success(),
                            json!({
                                "task_id": task_id.as_str(),
                                "run_id": run_id.as_str(),
                                "final_diff_path": patch_path,
                            }),
                        ));
                    }
                }
            }
        }

        let ticket = self.insert_ticket(
            task_id,
            &run_id,
            "validation failed",
            "Validation failed after all fake-provider attempts. What should change next?",
        )?;
        self.finish_run_stuck(&run_id, "validation failed after max attempts")?;
        self.store
            .transition_task(task_id, TaskStatus::Running, TaskStatus::Stuck, OWNER)?;
        self.store.release_task_lease(task_id, OWNER)?;
        Ok(CommandResult::with_data(
            CommandExit::stuck(format!("task {task_id} is stuck")),
            json!({
                "task_id": task_id.as_str(),
                "run_id": run_id.as_str(),
                "ticket_id": ticket.id.as_str(),
            }),
        ))
    }

    fn prompt(&self, task: &Task, resolution: Option<&TicketResolution>) -> String {
        let mut text = format!(
            "Task: {}\nGoal: {}\nRepo: {}\n",
            task.title,
            task.goal,
            self.repo.display()
        );
        if let Some(resolution) = resolution {
            let resolution_text =
                fs::read_to_string(&resolution.resolution_path).unwrap_or_default();
            text.push_str("\nTicket resolution:\n");
            text.push_str(&resolution_text);
        }
        self.redactor.redact(&text).text
    }

    fn write_run_artifact(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        kind: &str,
        content: &str,
    ) -> HarnessResult<String> {
        let dir = self
            .paths
            .run_artifact_dir(task_id.as_str(), run_id.as_str());
        fs::create_dir_all(&dir).map_err(io_error)?;
        let path = dir.join(format!(
            "{:03}_{kind}.txt",
            self.ids.lock().unwrap().artifact_sequence
        ));
        let redacted = self.redactor.redact(content).text;
        fs::write(&path, redacted.as_bytes()).map_err(io_error)?;
        let artifact = artifact(
            self.next_artifact_id(),
            task_id.clone(),
            Some(run_id.clone()),
            None,
            None,
            kind,
            path.to_string_lossy().into_owned(),
        );
        self.store.insert_artifact(artifact, OWNER)?;
        Ok(path.to_string_lossy().into_owned())
    }

    fn insert_attempt(
        &self,
        run_id: &RunId,
        attempt_number: u32,
        model: &str,
        status: AttemptStatus,
        prompt_path: Option<String>,
        response_path: Option<String>,
        patch_path: Option<String>,
        validation_log_path: Option<String>,
        validation_exit_code: Option<i32>,
        failure_reason: Option<String>,
        apply_error: Option<String>,
    ) -> HarnessResult<Attempt> {
        let attempt = Attempt {
            id: self.next_attempt_id(),
            run_id: run_id.clone(),
            attempt_number,
            provider: "fake-ollama".to_string(),
            model: model.to_string(),
            status,
            prompt_path,
            response_path,
            patch_path,
            validation_log_path,
            validation_exit_code,
            failure_reason,
            apply_error,
            started_at: now(),
            finished_at: Some(now()),
        };
        self.store.insert_attempt(attempt.clone(), OWNER)?;
        Ok(attempt)
    }

    fn insert_ticket(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        reason: &str,
        question: &str,
    ) -> HarnessResult<Ticket> {
        let ticket = Ticket {
            id: self.next_ticket_id(),
            task_id: task_id.clone(),
            run_id: run_id.clone(),
            status: TicketStatus::Open,
            blocked_on: "validation".to_string(),
            question: self.redactor.redact(question).text,
            reason: self.redactor.redact(reason).text,
            evidence_json: json!({
                "run_id": run_id.as_str(),
                "task_goal": self.store.get_task(task_id)?.goal,
                "redacted": true,
            })
            .to_string(),
            failure_fingerprint: format!("{}:{reason}", run_id.as_str()),
            created_at: now(),
            resolved_at: None,
        };
        self.store.create_or_get_ticket(ticket, OWNER)
    }

    fn finish_run_complete(&self, run_id: &RunId, final_diff_path: &str) -> HarnessResult<()> {
        self.store.update_run(
            run_id,
            Some(RunStatus::Running),
            RunUpdate {
                status: Some(RunStatus::Complete),
                current_phase: Some("complete".to_string()),
                finished_at: Some(now()),
                final_diff_path: Some(final_diff_path.to_string()),
                ..RunUpdate::default()
            },
            OWNER,
        )?;
        Ok(())
    }

    fn finish_run_stuck(&self, run_id: &RunId, reason: &str) -> HarnessResult<()> {
        self.store.update_run(
            run_id,
            Some(RunStatus::Running),
            RunUpdate {
                status: Some(RunStatus::Stuck),
                current_phase: Some("stuck".to_string()),
                finished_at: Some(now()),
                last_error: Some(self.redactor.redact(reason).text),
                ..RunUpdate::default()
            },
            OWNER,
        )?;
        Ok(())
    }

    fn fail_run_and_task(
        &self,
        task_id: &TaskId,
        run_id: &RunId,
        reason: &str,
    ) -> HarnessResult<()> {
        self.store.update_run(
            run_id,
            Some(RunStatus::Running),
            RunUpdate {
                status: Some(RunStatus::Failed),
                current_phase: Some("failed".to_string()),
                finished_at: Some(now()),
                last_error: Some(self.redactor.redact(reason).text),
                ..RunUpdate::default()
            },
            OWNER,
        )?;
        self.store
            .transition_task(task_id, TaskStatus::Running, TaskStatus::Failed, OWNER)?;
        Ok(())
    }
}

impl HarnessService for E2eService {
    fn create_task(
        &self,
        title: String,
        goal: String,
        validation_commands: Vec<String>,
    ) -> HarnessResult<Task> {
        let now = now();
        let task = Task {
            id: self.next_task_id(),
            title: self.redactor.redact(&title).text,
            goal: self.redactor.redact(&goal).text,
            status: TaskStatus::Ready,
            repo_root: self.repo.to_string_lossy().into_owned(),
            worktree_path: None,
            branch: None,
            base_ref: None,
            base_commit: None,
            last_seen_head: None,
            max_attempts: 3,
            lease_owner: None,
            lease_acquired_at: None,
            lease_expires_at: None,
            heartbeat_at: None,
            lock_version: 0,
            created_at: now.clone(),
            updated_at: now,
        };
        let validation_commands = validation_commands
            .into_iter()
            .map(|command| self.redactor.redact(&command).text)
            .collect();
        self.store.insert_task(task.clone(), validation_commands)?;
        Ok(task)
    }

    fn list_tasks(&self) -> HarnessResult<Vec<Task>> {
        self.store.list_tasks(None)
    }

    fn get_task(&self, task_id: &TaskId) -> HarnessResult<Task> {
        self.store.get_task(task_id)
    }

    fn run_task(&self, task_id: &TaskId, options: TaskRunOptions) -> HarnessResult<CommandResult> {
        assert_runtime_is_hermetic(&options.runtime);
        self.run_attempts(
            task_id,
            None,
            options.max_attempts.unwrap_or(3),
            options
                .model
                .unwrap_or_else(|| "fake-local-model".to_string()),
            None,
        )
    }

    fn list_tickets(&self) -> HarnessResult<Vec<Ticket>> {
        self.store.list_tickets(None, None)
    }

    fn get_ticket(&self, ticket_id: &TicketId) -> HarnessResult<Ticket> {
        self.store.get_ticket(ticket_id)
    }

    fn resolve_ticket(
        &self,
        ticket_id: &TicketId,
        options: TicketResolveOptions,
    ) -> HarnessResult<CommandResult> {
        assert_runtime_is_hermetic(&options.runtime);
        let ticket = self.store.get_ticket(ticket_id)?;
        self.store.acquire_task_lease(&ticket.task_id, OWNER)?;
        self.store.transition_ticket(
            ticket_id,
            TicketStatus::Open,
            TicketStatus::Resolving,
            OWNER,
        )?;

        let request_input = self.redactor.redact(&format!(
            "Resolve ticket.\nReason: {}\nQuestion: {}\nEvidence: {}",
            ticket.reason, ticket.question, ticket.evidence_json
        ));
        let model = options
            .model
            .unwrap_or_else(|| "fake-openai-model".to_string());
        let response = block_on_provider(self.openai.complete(ModelRequest {
            model: model.clone(),
            system: Some("Return concise ticket-resolution guidance.".to_string()),
            input: request_input.text,
            temperature: Some(0.0),
            max_output_tokens: Some(4096),
            metadata: BTreeMap::from([("ticket_id".to_string(), ticket_id.as_str().to_string())]),
        }))
        .map_err(provider_error)?;
        let resolution_text = self.redactor.redact(&response.text).text;

        let dir = self
            .paths
            .artifacts_dir
            .join(ticket.task_id.as_str())
            .join(ticket.run_id.as_str());
        fs::create_dir_all(&dir).map_err(io_error)?;
        let path = dir.join(format!("resolution_{}.txt", ticket_id.as_str()));
        fs::write(&path, resolution_text.as_bytes()).map_err(io_error)?;
        let resolution = TicketResolution {
            id: self.next_resolution_id(),
            ticket_id: ticket_id.clone(),
            provider: "fake-openai".to_string(),
            model: model.clone(),
            response_id: response.response_id,
            resolution_path: path.to_string_lossy().into_owned(),
            consumed_at: None,
            created_at: now(),
        };
        self.store
            .insert_ticket_resolution(resolution.clone(), OWNER)?;
        self.store.insert_artifact(
            artifact(
                self.next_artifact_id(),
                ticket.task_id.clone(),
                Some(ticket.run_id.clone()),
                None,
                Some(ticket_id.clone()),
                "ticket_resolution",
                resolution.resolution_path.clone(),
            ),
            OWNER,
        )?;
        self.store.release_task_lease(&ticket.task_id, OWNER)?;
        Ok(CommandResult::with_data(
            CommandExit::success(),
            json!({
                "ticket_id": ticket_id.as_str(),
                "resolution_id": resolution.id.as_str(),
                "response_id": resolution.response_id,
                "resolution_path": resolution.resolution_path,
            }),
        ))
    }

    fn resume_task(
        &self,
        task_id: &TaskId,
        options: ResumeTaskOptions,
    ) -> HarnessResult<CommandResult> {
        assert_runtime_is_hermetic(&options.runtime);
        let resolution = match options.ticket_id {
            Some(ticket_id) => self
                .store
                .latest_unconsumed_resolution_for_ticket(&ticket_id)?,
            None => self.store.latest_unconsumed_resolution(task_id)?,
        }
        .ok_or_else(|| HarnessError::Conflict("no unconsumed ticket resolution".to_string()))?;
        let parent = self.store.latest_run_for_task(task_id)?.map(|run| run.id);
        self.run_attempts(
            task_id,
            parent,
            options.max_attempts.unwrap_or(3),
            options
                .model
                .unwrap_or_else(|| "fake-local-model".to_string()),
            Some(resolution),
        )
    }
}

#[derive(Default)]
struct IdAllocator {
    task_sequence: usize,
    run_sequence: usize,
    attempt_sequence: usize,
    ticket_sequence: usize,
    resolution_sequence: usize,
    artifact_sequence: usize,
    event_sequence: usize,
}

impl IdAllocator {
    fn task(&mut self) -> TaskId {
        self.task_sequence += 1;
        TaskId::parse(id("task_", self.task_sequence)).unwrap()
    }

    fn run(&mut self) -> RunId {
        self.run_sequence += 1;
        RunId::parse(id("run_", self.run_sequence)).unwrap()
    }

    fn attempt(&mut self) -> AttemptId {
        self.attempt_sequence += 1;
        AttemptId::parse(id("att_", self.attempt_sequence)).unwrap()
    }

    fn ticket(&mut self) -> TicketId {
        self.ticket_sequence += 1;
        TicketId::parse(id("ticket_", self.ticket_sequence)).unwrap()
    }

    fn resolution(&mut self) -> TicketResolutionId {
        self.resolution_sequence += 1;
        TicketResolutionId::parse(id("res_", self.resolution_sequence)).unwrap()
    }

    fn artifact(&mut self) -> ArtifactId {
        self.artifact_sequence += 1;
        ArtifactId::parse(id("art_", self.artifact_sequence)).unwrap()
    }

    #[allow(dead_code)]
    fn event(&mut self) -> EventId {
        self.event_sequence += 1;
        EventId::parse(id("event_", self.event_sequence)).unwrap()
    }
}

fn id(prefix: &str, sequence: usize) -> String {
    const ALPHABET: &[u8] = b"123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let ch = ALPHABET
        .get(sequence.saturating_sub(1))
        .copied()
        .unwrap_or(b'Z') as char;
    format!("{prefix}01ARZ3NDEKTSV4RRFFQ69G5FA{ch}")
}

fn artifact(
    id: ArtifactId,
    task_id: TaskId,
    run_id: Option<RunId>,
    attempt_id: Option<AttemptId>,
    ticket_id: Option<TicketId>,
    kind: &str,
    path: String,
) -> Artifact {
    let bytes = fs::read(&path).unwrap_or_default();
    Artifact {
        id,
        task_id,
        run_id,
        attempt_id,
        ticket_id,
        kind: kind.to_string(),
        sha256: hash64_hex(&bytes),
        byte_len: bytes.len() as u64,
        path,
        created_at: now(),
    }
}

fn hash64_hex(bytes: &[u8]) -> String {
    sha256_bytes(bytes)
}

fn now() -> String {
    "2026-05-13T00:00:00Z".to_string()
}

fn assert_runtime_is_hermetic(runtime: &RuntimeOptions) {
    assert_eq!(runtime.output, OutputMode::Json);
}

fn command_stdout(command: &mut Command) -> HarnessResult<String> {
    let output = command.stderr(Stdio::piped()).output().map_err(io_error)?;
    if !output.status.success() {
        return Err(HarnessError::External(format!(
            "command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn apply_patch(repo: &Path, diff: &str) -> HarnessResult<()> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["apply", "-"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error)?;
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("git apply stdin")
            .write_all(diff.as_bytes())
            .map_err(io_error)?;
    }
    let output = child.wait_with_output().map_err(io_error)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(HarnessError::External(format!(
            "git apply failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

struct ValidationOutput {
    code: i32,
    text: String,
}

fn run_validation(repo: &Path, validation_command: &str) -> HarnessResult<ValidationOutput> {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(validation_command)
        .current_dir(repo)
        .output()
        .map_err(io_error)?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(ValidationOutput {
        code: output.status.code().unwrap_or(1),
        text,
    })
}

fn io_error(error: std::io::Error) -> HarnessError {
    HarnessError::External(error.to_string())
}

fn provider_error(error: harness::providers::ProviderError) -> HarnessError {
    HarnessError::External(format!(
        "provider {}: {}",
        error.kind.as_str(),
        error.message
    ))
}

fn assert_no_secret_in_service_outputs(service: &E2eService, outputs: &[&str]) {
    assert_no_secret_in_persisted_outputs(
        &service.store,
        &service.paths.state_file,
        &service.paths.artifacts_dir,
        outputs,
    );
}

fn assert_no_secret_in_persisted_outputs(
    store: &dyn TaskStore,
    state_file: &Path,
    artifacts_dir: &Path,
    outputs: &[&str],
) {
    for output in outputs {
        assert!(!output.contains("sk-testsecret"), "{output}");
    }

    let sqlite = fs::read(state_file).unwrap();
    assert!(!String::from_utf8_lossy(&sqlite).contains("sk-testsecret"));
    assert_no_secret_in_artifact_tree(artifacts_dir);

    for task in store.list_tasks(None).unwrap() {
        assert!(!format!("{task:?}").contains("sk-testsecret"));
        for event in store.list_events_for_task(&task.id).unwrap() {
            assert!(!format!("{event:?}").contains("sk-testsecret"));
        }
        for ticket in store.list_tickets(Some(&task.id), None).unwrap() {
            assert!(!format!("{ticket:?}").contains("sk-testsecret"));
            for resolution in store.list_ticket_resolutions(&ticket.id).unwrap() {
                assert!(!format!("{resolution:?}").contains("sk-testsecret"));
                let text = fs::read_to_string(&resolution.resolution_path).unwrap();
                assert!(!text.contains("sk-testsecret"));
            }
        }
        if let Some(run) = store.latest_run_for_task(&task.id).unwrap() {
            for artifact in store.list_artifacts_for_run(&run.id).unwrap() {
                assert!(!format!("{artifact:?}").contains("sk-testsecret"));
                let text = fs::read_to_string(&artifact.path).unwrap_or_default();
                assert!(!text.contains("sk-testsecret"), "{}", artifact.path);
            }
        }
    }
}

fn assert_no_secret_in_provider_requests(provider: &FakeModelProvider) {
    for request in provider.requests() {
        assert!(!format!("{request:?}").contains("sk-testsecret"));
        assert!(!request.input.contains("sk-testsecret"));
        if request.input.contains("Evidence includes") || request.input.contains("Do not leak") {
            assert!(request.input.contains("[REDACTED"));
        }
    }
}

fn assert_no_secret_in_artifact_tree(root: &Path) {
    if !root.exists() {
        return;
    }
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            assert_no_secret_in_artifact_tree(&path);
        } else {
            let bytes = fs::read(&path).unwrap();
            assert!(
                !String::from_utf8_lossy(&bytes).contains("sk-testsecret"),
                "{}",
                path.display()
            );
        }
    }
}

fn success_patch() -> &'static str {
    "```diff\ndiff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,5 +1,5 @@\n pub fn add(left: i32, right: i32) -> i32 {\n-    left - right\n+    left + right\n }\n \n #[cfg(test)]\n\n```"
}

fn passing_add_patch() -> &'static str {
    "```diff\ndiff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,5 +1,5 @@\n pub fn add(left: i32, right: i32) -> i32 {\n-    left + right\n+    left.checked_add(right).unwrap()\n }\n \n #[cfg(test)]\n\n```"
}

fn make_rust_success_fixture_pass(repo: &Path) {
    fs::write(
        repo.join("src/lib.rs"),
        r#"pub fn add(left: i32, right: i32) -> i32 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_numbers() {
        assert_eq!(add(2, 2), 4);
    }
}
"#,
    )
    .expect("write passing fixture lib.rs");
    let add = Command::new("git")
        .args(["add", "src/lib.rs"])
        .current_dir(repo)
        .output()
        .expect("git add passing fixture");
    assert!(add.status.success(), "{add:?}");
    let commit = Command::new("git")
        .args(["commit", "-m", "make fixture pass before objective"])
        .current_dir(repo)
        .output()
        .expect("git commit passing fixture");
    assert!(commit.status.success(), "{commit:?}");
}

fn objective_planner_json() -> String {
    serde_json::json!({
        "schema_version": 1,
        "objective": {
            "title": "Maintain Rust project",
            "summary": "Run a small validation-backed Rust maintenance objective.",
            "acceptance_criteria": ["cargo test passes"],
            "validation_commands": ["cargo test"]
        },
        "tasks": [
            {
                "task_key": "validate_project",
                "title": "Validate project",
                "goal": "Confirm the Rust project remains valid.",
                "validation": ["cargo test"],
                "depends_on": [],
                "owned_paths": ["src"],
                "parallel_group": "validation"
            }
        ],
        "risks": [],
        "final_verification": ["cargo test"]
    })
    .to_string()
}

fn objective_resolver_json() -> String {
    serde_json::json!({
        "schema_version": 1,
        "diagnosis": "The local worker should apply the addition fix and rerun the focused validation.",
        "recommended_steps": [
            "Apply the existing arithmetic patch.",
            "Rerun cargo test validate_project."
        ],
        "constraints": ["Do not change unrelated behavior."],
        "validation_focus": ["cargo test validate_project"]
    })
    .to_string()
}

fn secret_success_patch() -> String {
    format!(
        "```diff\ndiff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,5 +1,6 @@\n pub fn add(left: i32, right: i32) -> i32 {{\n-    left - right\n+    // {SECRET}\n+    left + right\n }}\n \n #[cfg(test)]\n\n```"
    )
}

fn still_failing_patch() -> &'static str {
    "```diff\ndiff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,5 +1,5 @@\n pub fn is_even(value: i32) -> bool {\n-    value % 2 == 1\n+    value % 2 != 0\n }\n \n #[cfg(test)]\n\n```"
}

fn resume_patch() -> &'static str {
    "```diff\ndiff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,5 +1,5 @@\n pub fn normalize(input: &str) -> String {\n-    input.to_string()\n+    input.trim().to_lowercase()\n }\n \n #[cfg(test)]\n\n```"
}

#[allow(dead_code)]
fn event(id: EventId, task_id: TaskId, run_id: RunId, message: &str) -> Event {
    Event {
        id,
        task_id: Some(task_id),
        run_id: Some(run_id),
        kind: "e2e.event".to_string(),
        level: EventLevel::Info,
        message: message.to_string(),
        artifact_path: None,
        created_at: now(),
    }
}
