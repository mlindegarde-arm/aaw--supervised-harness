#[path = "support/fixtures.rs"]
mod fixtures;

use fixtures::{FixtureKind, create_fixture};
use std::process::Command;

#[test]
fn fixture_skeleton_creates_expected_repositories() {
    let temp = tempfile::tempdir().expect("tempdir");

    let success = create_fixture(temp.path(), FixtureKind::RustSuccess);
    let stuck = create_fixture(temp.path(), FixtureKind::RustValidationFailsThenStuck);
    let resume = create_fixture(temp.path(), FixtureKind::RustResumeAfterTicket);
    let not_git = create_fixture(temp.path(), FixtureKind::NotGitRepo);

    assert_eq!(success.kind.name(), "rust_success");
    assert!(success.path.join(".git").exists());
    assert!(success.path.join(".gitignore").exists());
    assert!(success.path.join("Cargo.toml").exists());
    assert!(success.path.join("src/lib.rs").exists());
    assert!(success.path.join("HARNESS_FIXTURE.md").exists());

    assert_eq!(stuck.kind.name(), "rust_validation_fails_then_stuck");
    assert_eq!(
        stuck.kind.scenario(),
        "local_validation_retries_then_stuck_ticket"
    );
    assert!(stuck.path.join(".git").exists());
    assert!(stuck.path.join("Cargo.toml").exists());

    assert_eq!(resume.kind.name(), "rust_resume_after_ticket");
    assert_eq!(
        resume.kind.scenario(),
        "stuck_then_openai_resolution_then_resume_success"
    );
    assert!(resume.path.join(".git").exists());
    assert!(resume.path.join("Cargo.toml").exists());

    assert_eq!(not_git.kind.name(), "not_git_repo");
    assert!(!not_git.path.join(".git").exists());
    assert!(not_git.path.join("README.md").exists());
}

#[test]
fn rust_fixtures_start_with_intentionally_failing_tests() {
    let temp = tempfile::tempdir().expect("tempdir");

    for kind in [
        FixtureKind::RustSuccess,
        FixtureKind::RustValidationFailsThenStuck,
        FixtureKind::RustResumeAfterTicket,
    ] {
        let fixture = create_fixture(temp.path(), kind);
        let compile = Command::new("cargo")
            .args(["test", "--no-run"])
            .current_dir(&fixture.path)
            .output()
            .expect("run fixture cargo test --no-run");
        assert!(
            compile.status.success(),
            "{} should compile before validation fails\nstdout:\n{}\nstderr:\n{}",
            kind.name(),
            String::from_utf8_lossy(&compile.stdout),
            String::from_utf8_lossy(&compile.stderr)
        );

        let output = Command::new("cargo")
            .arg("test")
            .current_dir(&fixture.path)
            .output()
            .expect("run fixture cargo test");

        assert!(
            !output.status.success(),
            "{} should begin with a failing validation target",
            kind.name()
        );

        let git_status = Command::new("git")
            .args(["status", "--porcelain=v1"])
            .current_dir(&fixture.path)
            .output()
            .expect("run fixture git status");
        assert!(
            git_status.status.success(),
            "{} git status should run",
            kind.name()
        );
        assert_eq!(
            String::from_utf8_lossy(&git_status.stdout),
            "",
            "{} should stay clean after validation output",
            kind.name()
        );
    }
}
