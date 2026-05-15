use crate::domain::ObjectiveValidationReviewStatus;
use crate::{HarnessError, HarnessResult};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationCommandClassification {
    Trusted,
    NeedsReview,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCommandReview {
    pub command: String,
    pub argv: Vec<String>,
    pub classification: ValidationCommandClassification,
    pub reasons: Vec<String>,
}

impl ValidationCommandReview {
    pub fn review_status(&self) -> ObjectiveValidationReviewStatus {
        match self.classification {
            ValidationCommandClassification::Trusted => ObjectiveValidationReviewStatus::Trusted,
            ValidationCommandClassification::NeedsReview => {
                ObjectiveValidationReviewStatus::NeedsReview
            }
            ValidationCommandClassification::Rejected => ObjectiveValidationReviewStatus::Rejected,
        }
    }

    pub fn executable_argv(&self) -> Option<&[String]> {
        (self.classification == ValidationCommandClassification::Trusted)
            .then_some(self.argv.as_slice())
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ValidationCommandPolicy;

impl ValidationCommandPolicy {
    pub fn new() -> Self {
        Self
    }

    pub fn classify(&self, command: impl AsRef<str>) -> ValidationCommandReview {
        let command = command.as_ref();
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return rejected(command, Vec::new(), "validation command is empty");
        }
        if trimmed.contains('\0') {
            return rejected(command, Vec::new(), "validation command contains NUL");
        }
        if trimmed.chars().any(|ch| matches!(ch, '\n' | '\r')) {
            return rejected(
                command,
                Vec::new(),
                "validation command must be a single command line",
            );
        }
        if let Some(reason) = shell_metacharacter_reason(trimmed) {
            return rejected(command, Vec::new(), reason);
        }

        let parsed = match parse_argv_like(trimmed) {
            Ok(parsed) => parsed,
            Err(reason) => return rejected(command, Vec::new(), reason),
        };
        if parsed.argv.is_empty() {
            return rejected(command, Vec::new(), "validation command is empty");
        }

        let mut rejected_reasons = Vec::new();
        let mut review_reasons = Vec::new();
        let argv = parsed.argv;

        if is_env_assignment(&argv[0]) {
            rejected_reasons.push(
                "environment assignment prefixes are not allowed in planner validation commands"
                    .to_string(),
            );
        }
        if parsed.used_shell_quoting {
            review_reasons.push(
                "shell quoting or escaping requires review before the command can run".to_string(),
            );
        }

        inspect_argv_paths(&argv, &mut rejected_reasons);
        inspect_program_policy(&argv, &mut rejected_reasons, &mut review_reasons);

        if !rejected_reasons.is_empty() {
            return ValidationCommandReview {
                command: command.to_string(),
                argv,
                classification: ValidationCommandClassification::Rejected,
                reasons: rejected_reasons,
            };
        }
        if !review_reasons.is_empty() {
            return ValidationCommandReview {
                command: command.to_string(),
                argv,
                classification: ValidationCommandClassification::NeedsReview,
                reasons: dedupe(review_reasons),
            };
        }
        if is_trusted_validation(&argv) {
            return ValidationCommandReview {
                command: command.to_string(),
                argv,
                classification: ValidationCommandClassification::Trusted,
                reasons: Vec::new(),
            };
        }

        needs_review(
            command,
            argv,
            "command is argv-like but is not in the trusted validation allowlist",
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoPath {
    raw: String,
    relative: PathBuf,
}

impl RepoPath {
    pub fn validate(repo_root: impl AsRef<Path>, value: impl AsRef<str>) -> HarnessResult<Self> {
        Self::validate_with_options(repo_root, value, RepoPathOptions::default())
    }

    pub fn validate_lexical(value: impl AsRef<str>) -> HarnessResult<Self> {
        Self::validate_lexical_with_options(value, RepoPathOptions::default())
    }

    pub fn validate_lexical_with_options(
        value: impl AsRef<str>,
        options: RepoPathOptions,
    ) -> HarnessResult<Self> {
        let raw = value.as_ref();
        let relative = lexical_repo_path(raw, options)?;
        Ok(Self {
            raw: raw.to_string(),
            relative,
        })
    }

    pub fn validate_with_options(
        repo_root: impl AsRef<Path>,
        value: impl AsRef<str>,
        options: RepoPathOptions,
    ) -> HarnessResult<Self> {
        let repo_root = canonicalize_existing(repo_root.as_ref())?;
        let raw = value.as_ref();
        let path = Self::validate_lexical_with_options(raw, options)?;
        let relative = path.relative;
        let resolved_parent = canonicalize_existing_parent(&repo_root, &relative)?;
        if !resolved_parent.starts_with(&repo_root) {
            return Err(policy_error(format!(
                "repo path {raw:?} escapes repository through a symlink"
            )));
        }
        Ok(Self {
            raw: raw.to_string(),
            relative,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn relative_path(&self) -> &Path {
        &self.relative
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoPathOptions {
    pub approved_harness_prefixes: Vec<PathBuf>,
}

impl Default for RepoPathOptions {
    fn default() -> Self {
        Self {
            approved_harness_prefixes: Vec::new(),
        }
    }
}

impl RepoPathOptions {
    pub fn allow_harness_prefix(mut self, prefix: impl Into<PathBuf>) -> Self {
        self.approved_harness_prefixes.push(prefix.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedCommand {
    argv: Vec<String>,
    used_shell_quoting: bool,
}

fn rejected(
    command: &str,
    argv: Vec<String>,
    reason: impl Into<String>,
) -> ValidationCommandReview {
    ValidationCommandReview {
        command: command.to_string(),
        argv,
        classification: ValidationCommandClassification::Rejected,
        reasons: vec![reason.into()],
    }
}

fn needs_review(
    command: &str,
    argv: Vec<String>,
    reason: impl Into<String>,
) -> ValidationCommandReview {
    ValidationCommandReview {
        command: command.to_string(),
        argv,
        classification: ValidationCommandClassification::NeedsReview,
        reasons: vec![reason.into()],
    }
}

fn shell_metacharacter_reason(command: &str) -> Option<&'static str> {
    if command.contains("$(") {
        return Some("command substitution is not allowed in validation commands");
    }
    if command.contains("&&") || command.contains("||") {
        return Some("shell command chaining is not allowed in validation commands");
    }
    for ch in command.chars() {
        match ch {
            '|' => return Some("shell pipes are not allowed in validation commands"),
            '>' | '<' => return Some("shell redirection is not allowed in validation commands"),
            ';' => return Some("shell command separators are not allowed in validation commands"),
            '&' => return Some("background execution is not allowed in validation commands"),
            '`' => {
                return Some("backtick command substitution is not allowed in validation commands");
            }
            _ => {}
        }
    }
    None
}

fn parse_argv_like(command: &str) -> Result<ParsedCommand, &'static str> {
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut used_shell_quoting = false;
    let mut token_started = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else if ch == '\\' {
                used_shell_quoting = true;
                if let Some(next) = chars.next() {
                    current.push(next);
                } else {
                    return Err("trailing shell escape is not allowed");
                }
            } else {
                current.push(ch);
            }
            continue;
        }

        if ch.is_whitespace() {
            if token_started {
                argv.push(std::mem::take(&mut current));
                token_started = false;
            }
            continue;
        }
        token_started = true;
        match ch {
            '\'' | '"' => {
                used_shell_quoting = true;
                quote = Some(ch);
            }
            '\\' => {
                used_shell_quoting = true;
                if let Some(next) = chars.next() {
                    current.push(next);
                } else {
                    return Err("trailing shell escape is not allowed");
                }
            }
            _ => current.push(ch),
        }
    }

    if quote.is_some() {
        return Err("unterminated shell quote in validation command");
    }
    if token_started {
        argv.push(current);
    }

    Ok(ParsedCommand {
        argv,
        used_shell_quoting,
    })
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .bytes()
            .next()
            .is_some_and(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && name
            .bytes()
            .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'_' | b'0'..=b'9'))
}

fn inspect_argv_paths(argv: &[String], rejected_reasons: &mut Vec<String>) {
    for (idx, arg) in argv.iter().enumerate() {
        for inspected in path_payloads(arg) {
            if has_dangerous_platform_prefix(inspected) || looks_absolute(inspected) {
                rejected_reasons.push(format!(
                    "argument {arg:?} uses an absolute or platform path"
                ));
            }
            if contains_parent_component(inspected) {
                rejected_reasons.push(format!("argument {arg:?} contains path traversal"));
            }
            if contains_component(inspected, ".git") {
                rejected_reasons.push(format!("argument {arg:?} enters .git"));
            }
            if contains_component(inspected, ".harness") {
                rejected_reasons.push(format!("argument {arg:?} enters .harness"));
            }
            if idx == 0 {
                if looks_like_program_path(inspected) {
                    rejected_reasons.push(format!(
                        "program path {arg:?} is out of scope for planner validation commands"
                    ));
                } else if looks_like_script_path(inspected) {
                    rejected_reasons.push(format!(
                        "script path {arg:?} is out of scope for planner validation commands"
                    ));
                }
            }
        }
    }
}

fn inspect_program_policy(
    argv: &[String],
    rejected_reasons: &mut Vec<String>,
    review_reasons: &mut Vec<String>,
) {
    let program = basename(&argv[0]);
    let subcommand = argv.get(1).map(String::as_str);

    if matches!(program, "sh" | "bash" | "zsh" | "dash" | "fish")
        && subcommand.is_some_and(|arg| arg == "-c" || arg == "-lc")
    {
        rejected_reasons.push("shell interpreters with -c are not allowed".to_string());
    }

    if matches!(
        program,
        "curl" | "wget" | "ssh" | "scp" | "sftp" | "rsync" | "nc" | "netcat"
    ) {
        rejected_reasons.push(format!("network tool {program:?} is not allowed"));
    }

    if matches!(
        program,
        "rm" | "rmdir" | "mv" | "dd" | "chmod" | "chown" | "mkfs"
    ) {
        rejected_reasons.push(format!("destructive command {program:?} is not allowed"));
    }

    if program == "git" && matches!(subcommand, Some("reset" | "clean")) {
        rejected_reasons.push(format!(
            "destructive git subcommand {:?} is not allowed",
            subcommand.unwrap()
        ));
    }
    if program == "git" && matches!(subcommand, Some("push" | "fetch" | "pull" | "clone")) {
        rejected_reasons.push(format!(
            "network git subcommand {:?} is not allowed",
            subcommand.unwrap()
        ));
    }
    if program == "cargo"
        && (matches!(subcommand, Some("fix")) || argv.iter().skip(2).any(|arg| arg == "--fix"))
    {
        rejected_reasons.push("mutating cargo fix commands are not allowed".to_string());
    }

    if is_package_mutation(program, subcommand) {
        rejected_reasons.push(format!(
            "package mutation command {:?} {:?} is not allowed for validation",
            program, subcommand
        ));
    }

    if program == "make" || program == "just" {
        review_reasons.push(format!(
            "{program} targets are project-defined and require review before execution"
        ));
    }
}

fn is_package_mutation(program: &str, subcommand: Option<&str>) -> bool {
    matches!(
        (program, subcommand),
        ("cargo", Some("add" | "install" | "publish" | "update"))
            | ("npm", Some("install" | "i" | "add" | "update" | "publish"))
            | ("pnpm", Some("install" | "i" | "add" | "update" | "publish"))
            | ("yarn", Some("install" | "add" | "upgrade" | "publish"))
            | ("pip", Some("install"))
            | ("pip3", Some("install"))
            | ("go", Some("get" | "install"))
    )
}

fn is_trusted_validation(argv: &[String]) -> bool {
    let program = basename(&argv[0]);
    match program {
        "cargo" => trusted_cargo(argv),
        "go" => argv.get(1).is_some_and(|arg| arg == "test"),
        "npm" => argv.get(1).is_some_and(|arg| arg == "test"),
        "pytest" => true,
        _ => false,
    }
}

fn trusted_cargo(argv: &[String]) -> bool {
    match argv.get(1).map(String::as_str) {
        Some("test" | "check") => true,
        Some("fmt") => argv.iter().any(|arg| arg == "--check"),
        Some("clippy") => true,
        _ => false,
    }
}

fn basename(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

fn looks_like_script_path(arg: &str) -> bool {
    (arg.starts_with("./") || arg.starts_with("../") || arg.contains('/')) && arg.ends_with(".sh")
}

fn looks_like_program_path(arg: &str) -> bool {
    arg.contains('/') || arg.contains('\\') || arg.starts_with('.')
}

fn path_payloads(arg: &str) -> impl Iterator<Item = &str> {
    std::iter::once(arg).chain(arg.split_once('=').map(|(_, value)| value))
}

fn has_dangerous_platform_prefix(value: &str) -> bool {
    value.starts_with('~')
        || value.starts_with("\\\\")
        || (value.len() >= 2
            && value.as_bytes()[1] == b':'
            && value.as_bytes()[0].is_ascii_alphabetic())
}

fn looks_absolute(value: &str) -> bool {
    value.starts_with('/')
}

fn contains_parent_component(value: &str) -> bool {
    normalized_components(value).any(|component| component == "..")
}

fn contains_component(value: &str, wanted: &str) -> bool {
    normalized_components(value).any(|component| component == wanted)
}

fn normalized_components(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
}

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    for value in values {
        if !deduped.contains(&value) {
            deduped.push(value);
        }
    }
    deduped
}

fn lexical_repo_path(raw: &str, options: RepoPathOptions) -> HarnessResult<PathBuf> {
    if raw.trim().is_empty() {
        return Err(policy_error("repo path must not be empty"));
    }
    if raw.contains('\0') {
        return Err(policy_error(format!("repo path {raw:?} contains NUL")));
    }
    if has_dangerous_platform_prefix(raw) {
        return Err(policy_error(format!(
            "repo path {raw:?} uses a dangerous platform prefix"
        )));
    }

    let normalized = raw.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(policy_error(format!("repo path {raw:?} must be relative")));
    }

    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                if part == OsStr::new(".git") {
                    return Err(policy_error(format!("repo path {raw:?} enters .git")));
                }
                relative.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(policy_error(format!(
                    "repo path {raw:?} escapes repository"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(policy_error(format!("repo path {raw:?} must be relative")));
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(policy_error("repo path must name an in-repository path"));
    }
    if contains_harness_component(&relative) && !is_approved_harness_path(&relative, &options) {
        return Err(policy_error(format!(
            "repo path {raw:?} enters unapproved .harness state"
        )));
    }
    Ok(relative)
}

fn contains_harness_component(path: &Path) -> bool {
    path.components().any(
        |component| matches!(component, Component::Normal(part) if part == OsStr::new(".harness")),
    )
}

fn is_approved_harness_path(path: &Path, options: &RepoPathOptions) -> bool {
    options
        .approved_harness_prefixes
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

fn canonicalize_existing(path: &Path) -> HarnessResult<PathBuf> {
    fs::canonicalize(path).map_err(|error| {
        policy_error(format!(
            "failed to canonicalize {}: {error}",
            path.display()
        ))
    })
}

fn canonicalize_existing_parent(repo_root: &Path, relative: &Path) -> HarnessResult<PathBuf> {
    let full_path = repo_root.join(relative);
    let existing = if full_path.exists() {
        full_path.as_path()
    } else {
        nearest_existing_parent(&full_path)?
    };
    canonicalize_existing(existing)
}

fn nearest_existing_parent(path: &Path) -> HarnessResult<&Path> {
    let mut current = path.parent();
    while let Some(parent) = current {
        if parent.exists() {
            return Ok(parent);
        }
        current = parent.parent();
    }
    Err(policy_error(format!(
        "repo path {} has no existing parent",
        path.display()
    )))
}

fn policy_error(message: impl Into<String>) -> HarnessError {
    HarnessError::SecurityPolicy(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn classify(command: &str) -> ValidationCommandReview {
        ValidationCommandPolicy::new().classify(command)
    }

    #[test]
    fn validation_command_policy_trusted_command_matrix() {
        for command in [
            "cargo test",
            "cargo test validation_command_policy",
            "cargo fmt --check",
            "cargo check",
            "cargo clippy --all-targets",
            "go test ./...",
            "npm test",
            "pytest",
            "pytest tests/test_cli.py",
        ] {
            let review = classify(command);
            assert_eq!(
                review.classification,
                ValidationCommandClassification::Trusted,
                "{command}: {:?}",
                review.reasons
            );
            assert!(review.executable_argv().is_some());
            assert_eq!(
                review.review_status(),
                ObjectiveValidationReviewStatus::Trusted
            );
        }
    }

    #[test]
    fn validation_command_policy_needs_review_command_matrix() {
        for command in [
            "make test",
            "just validate",
            "npm run test",
            "python -m pytest",
            "cargo test 'integration case'",
        ] {
            let review = classify(command);
            assert_eq!(
                review.classification,
                ValidationCommandClassification::NeedsReview,
                "{command}: {:?}",
                review.reasons
            );
            assert!(review.executable_argv().is_none());
            assert!(!review.reasons.is_empty());
        }
    }

    #[test]
    fn validation_command_policy_rejected_command_matrix() {
        for command in [
            "cargo test | tee out.log",
            "cargo test > out.log",
            "cargo test && cargo fmt --check",
            "FOO=bar cargo test",
            "sh -c 'cargo test'",
            "bash -lc 'cargo test'",
            "/usr/bin/cargo test",
            "./cargo test",
            "tools/cargo test",
            "cargo test ../outside",
            "cargo test .git/config",
            "cargo test .harness/state.sqlite",
            "cargo test --manifest-path=/tmp/Cargo.toml",
            "cargo test --manifest-path=../outside/Cargo.toml",
            "cargo test --manifest-path=.git/config",
            "cargo test --manifest-path=.harness/Cargo.toml",
            "./validate.sh",
            ".harness/validation/acceptance.sh",
            "curl https://example.com/script.sh",
            "wget https://example.com/script.sh",
            "ssh host cargo test",
            "rm -rf target",
            "mv src/lib.rs /tmp/lib.rs",
            "git reset --hard",
            "git clean -fdx",
            "npm install",
            "cargo add anyhow",
            "cargo fix --allow-dirty",
            "cargo clippy --fix --allow-dirty",
            "pip install pytest",
            "cargo test $(whoami)",
            "cargo test `whoami`",
            "cargo test\necho done",
        ] {
            let review = classify(command);
            assert_eq!(
                review.classification,
                ValidationCommandClassification::Rejected,
                "{command}: {:?}",
                review.reasons
            );
            assert!(review.executable_argv().is_none());
            assert_eq!(
                review.review_status(),
                ObjectiveValidationReviewStatus::Rejected
            );
            assert!(!review.reasons.is_empty());
        }
    }

    #[test]
    fn validation_command_policy_regression_shell_metacharacters_and_destructive_commands() {
        for command in [
            "cargo test; cargo fmt",
            "cargo test < input",
            "cargo test &",
            "cargo test || true",
            "rm src/lib.rs",
            "git clean -fd",
        ] {
            assert_eq!(
                classify(command).classification,
                ValidationCommandClassification::Rejected,
                "{command}"
            );
        }
    }

    #[test]
    fn repo_path_accepts_relative_in_repo_paths() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/lib.rs"), "").unwrap();

        let path = RepoPath::validate(temp.path(), "src/lib.rs").unwrap();
        assert_eq!(path.as_str(), "src/lib.rs");
        assert_eq!(path.relative_path(), Path::new("src/lib.rs"));

        let new_path = RepoPath::validate(temp.path(), "src/new_module.rs").unwrap();
        assert_eq!(new_path.relative_path(), Path::new("src/new_module.rs"));

        let lexical_path = RepoPath::validate_lexical("src/future.rs").unwrap();
        assert_eq!(lexical_path.relative_path(), Path::new("src/future.rs"));
    }

    #[test]
    fn repo_path_rejects_unsafe_lexical_paths() {
        let temp = tempdir().unwrap();
        for path in [
            "../outside",
            "src/../../outside",
            "/tmp/outside",
            "C:\\repo\\src",
            "\\\\server\\share",
            "~/repo/src",
            ".git/config",
            "src/.git/config",
            ".harness/state.sqlite",
            "src/lib.rs\0",
        ] {
            assert!(RepoPath::validate(temp.path(), path).is_err(), "{path}");
        }
    }

    #[test]
    fn repo_path_allows_approved_harness_prefixes() {
        let temp = tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".harness/validation")).unwrap();
        let options = RepoPathOptions::default().allow_harness_prefix(".harness/validation");

        let path =
            RepoPath::validate_with_options(temp.path(), ".harness/validation/result.log", options)
                .unwrap();
        assert_eq!(
            path.relative_path(),
            Path::new(".harness/validation/result.log")
        );
    }

    #[cfg(unix)]
    #[test]
    fn repo_path_rejects_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        symlink(outside.path(), temp.path().join("linked-out")).unwrap();

        let result = RepoPath::validate(temp.path(), "linked-out/file.txt");
        assert!(result.is_err());
    }
}
