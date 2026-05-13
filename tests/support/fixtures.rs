use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureKind {
    RustSuccess,
    RustValidationFailsThenStuck,
    RustResumeAfterTicket,
    NotGitRepo,
}

impl FixtureKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::RustSuccess => "rust_success",
            Self::RustValidationFailsThenStuck => "rust_validation_fails_then_stuck",
            Self::RustResumeAfterTicket => "rust_resume_after_ticket",
            Self::NotGitRepo => "not_git_repo",
        }
    }

    pub fn scenario(self) -> &'static str {
        match self {
            Self::RustSuccess => "local_success_after_one_patch",
            Self::RustValidationFailsThenStuck => "local_validation_retries_then_stuck_ticket",
            Self::RustResumeAfterTicket => "stuck_then_openai_resolution_then_resume_success",
            Self::NotGitRepo => "repo_discovery_failure",
        }
    }
}

#[derive(Debug)]
pub struct FixtureRepo {
    #[allow(dead_code)]
    pub kind: FixtureKind,
    pub path: PathBuf,
}

pub fn create_fixture(root: &Path, kind: FixtureKind) -> FixtureRepo {
    let path = root.join(kind.name());
    fs::create_dir_all(&path).expect("create fixture directory");

    match kind {
        FixtureKind::NotGitRepo => write_not_git_repo(&path),
        FixtureKind::RustSuccess
        | FixtureKind::RustValidationFailsThenStuck
        | FixtureKind::RustResumeAfterTicket => {
            write_rust_project(&path, kind);
            init_git_repo(&path);
        }
    }

    FixtureRepo { kind, path }
}

fn write_not_git_repo(path: &Path) {
    fs::write(
        path.join("README.md"),
        "# Not a git repository\n\nUsed to assert repository discovery failures.\n",
    )
    .expect("write not-git README");
}

fn write_rust_project(path: &Path, kind: FixtureKind) {
    fs::create_dir_all(path.join("src")).expect("create src directory");
    fs::write(
        path.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{}"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
            kind.name().replace('_', "-")
        ),
    )
    .expect("write fixture Cargo.toml");
    fs::write(
        path.join(".gitignore"),
        "/target/\n/Cargo.lock\n/.harness/\n",
    )
    .expect("write fixture gitignore");

    let body = match kind {
        FixtureKind::RustSuccess => {
            r#"pub fn add(left: i32, right: i32) -> i32 {
    left - right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_numbers() {
        assert_eq!(add(2, 2), 4);
    }
}
"#
        }
        FixtureKind::RustValidationFailsThenStuck => {
            r#"pub fn is_even(value: i32) -> bool {
    value % 2 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_even_numbers() {
        assert!(is_even(2));
    }
}
"#
        }
        FixtureKind::RustResumeAfterTicket => {
            r#"pub fn normalize(input: &str) -> String {
    input.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_lowercases() {
        assert_eq!(normalize("  Hello  "), "hello");
    }
}
"#
        }
        FixtureKind::NotGitRepo => unreachable!("not-git fixture is not a Rust project"),
    };

    fs::write(path.join("src/lib.rs"), body).expect("write fixture lib.rs");
    fs::write(
        path.join("README.md"),
        format!("# {}\n\nHermetic harness fixture.\n", kind.name()),
    )
    .expect("write fixture README");
    fs::write(
        path.join("HARNESS_FIXTURE.md"),
        format!(
            "# {}\n\nScenario: {}\n\nValidation command: `cargo test`\n",
            kind.name(),
            kind.scenario()
        ),
    )
    .expect("write fixture scenario metadata");
}

fn init_git_repo(path: &Path) {
    run_git(path, &["init"]);
    run_git(
        path,
        &["config", "user.email", "harness-fixture@example.invalid"],
    );
    run_git(path, &["config", "user.name", "Harness Fixture"]);
    run_git(path, &["add", "."]);
    run_git(path, &["commit", "-m", "initial fixture"]);
}

fn run_git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .expect("run git");

    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
