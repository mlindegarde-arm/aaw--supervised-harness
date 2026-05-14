use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use harness::config::{ENV_ALLOW_UNTRUSTED_PROVIDER_URL, ENV_OLLAMA_BASE_URL, ENV_OPENAI_BASE_URL};

#[derive(Debug, Clone)]
pub struct BinaryHarness {
    bin: PathBuf,
    current_dir: Option<PathBuf>,
    inherited_env: BTreeMap<String, String>,
    env: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct BinaryOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl BinaryHarness {
    pub fn new() -> Self {
        Self {
            bin: PathBuf::from(env!("CARGO_BIN_EXE_harness")),
            current_dir: None,
            inherited_env: BTreeMap::new(),
            env: BTreeMap::new(),
        }
    }

    pub fn current_dir(mut self, current_dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(current_dir.into());
        self
    }

    pub fn inherited_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.inherited_env.insert(key.into(), value.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn run<I, S>(&self, args: I) -> BinaryOutput
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut command = Command::new(&self.bin);
        command.args(args.into_iter().map(|arg| arg.as_ref().to_string()));
        command.env_clear();
        command.envs(self.sanitized_inherited_env());
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }

        let output = command.output().expect("run harness binary");
        BinaryOutput {
            status: output.status,
            stdout: String::from_utf8(output.stdout).expect("binary stdout is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("binary stderr is UTF-8"),
        }
    }

    fn sanitized_inherited_env(&self) -> BTreeMap<String, String> {
        let mut env = std::env::vars()
            .filter(|(key, _)| !is_provider_override_env(key))
            .collect::<BTreeMap<_, _>>();
        env.extend(self.inherited_env.clone());
        env.retain(|key, _| !is_provider_override_env(key));
        env
    }

    pub fn init_repo(&self, repo: &Path) -> BinaryOutput {
        self.run([
            "--repo",
            repo.to_str().expect("repo path is UTF-8"),
            "--output",
            "json",
            "init",
        ])
    }

    pub fn init_repo_json(&self, repo: &Path) -> serde_json::Value {
        let output = self.init_repo(repo);
        assert!(
            output.status.success(),
            "harness init failed\nstdout:\n{}\nstderr:\n{}",
            output.stdout,
            output.stderr
        );
        assert!(
            output.stderr.is_empty(),
            "harness init wrote stderr:\n{}",
            output.stderr
        );
        serde_json::from_str(output.stdout.trim()).expect("init output is JSON")
    }
}

impl Default for BinaryHarness {
    fn default() -> Self {
        Self::new()
    }
}

fn is_provider_override_env(key: &str) -> bool {
    matches!(
        key,
        ENV_OLLAMA_BASE_URL
            | ENV_OPENAI_BASE_URL
            | ENV_ALLOW_UNTRUSTED_PROVIDER_URL
            | "ARM_OPENAI_API_KEY"
            | "OPENAI_API_KEY"
            | "OLLAMA_HOST"
            | "OLLAMA_API_KEY"
    ) || key.starts_with("HARNESS_OLLAMA_")
        || key.starts_with("HARNESS_OPENAI_")
        || key.starts_with("OPENAI_")
        || key.starts_with("OLLAMA_")
}
