#[derive(Debug, Clone, PartialEq)]
pub struct HarnessConfig {
    pub workspace: WorkspaceConfig,
    pub command: CommandConfig,
    pub orchestrator: OrchestratorConfig,
    pub providers: ProvidersConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceConfig {
    pub state_dir: String,
    pub worktree_root: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandConfig {
    pub shell_path: String,
    pub non_interactive_stdin: bool,
    pub kill_process_group_on_timeout: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrchestratorConfig {
    pub max_attempts: u32,
    pub max_invalid_responses: u32,
    pub max_provider_failures: u32,
    pub max_escalation_cycles: u32,
    pub validation_timeout_seconds: u64,
    pub max_validation_output_bytes: u64,
    pub max_patch_bytes: u64,
    pub max_files_changed: u32,
    pub max_total_runtime_seconds: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProvidersConfig {
    pub ollama: OllamaConfig,
    pub openai: OpenAiConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OllamaConfig {
    pub base_url: String,
    pub default_model: String,
    pub connect_timeout_seconds: u64,
    pub timeout_seconds: u64,
    pub max_retries: u32,
    pub retry_backoff_ms: u64,
    pub num_ctx: u32,
    pub num_predict: u32,
    pub temperature: f32,
    pub seed: u32,
    pub keep_alive: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiConfig {
    pub base_url: String,
    pub api_key_env: String,
    pub fallback_api_key_env: String,
    pub default_model: String,
    pub connect_timeout_seconds: u64,
    pub timeout_seconds: u64,
    pub max_retries: u32,
    pub retry_backoff_ms: u64,
    pub max_output_tokens: u32,
    pub allow_untrusted_provider_url: bool,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            workspace: WorkspaceConfig {
                state_dir: ".harness".to_string(),
                worktree_root: "../.harness-worktrees".to_string(),
            },
            command: CommandConfig {
                shell_path: "/bin/sh".to_string(),
                non_interactive_stdin: true,
                kill_process_group_on_timeout: true,
            },
            orchestrator: OrchestratorConfig {
                max_attempts: 3,
                max_invalid_responses: 2,
                max_provider_failures: 2,
                max_escalation_cycles: 1,
                validation_timeout_seconds: 120,
                max_validation_output_bytes: 65_536,
                max_patch_bytes: 131_072,
                max_files_changed: 20,
                max_total_runtime_seconds: 900,
            },
            providers: ProvidersConfig {
                ollama: OllamaConfig {
                    base_url: "http://localhost:11434".to_string(),
                    default_model: "maternion/strand-rust-coder:latest".to_string(),
                    connect_timeout_seconds: 10,
                    timeout_seconds: 120,
                    max_retries: 1,
                    retry_backoff_ms: 500,
                    num_ctx: 8192,
                    num_predict: 2048,
                    temperature: 0.0,
                    seed: 42,
                    keep_alive: "5m".to_string(),
                },
                openai: OpenAiConfig {
                    base_url: "https://openai-api-proxy.geo.arm.com/api/providers/openai-us/v1"
                        .to_string(),
                    api_key_env: "OPENAI_API_KEY".to_string(),
                    fallback_api_key_env: "ARM_OPENAI_API_KEY".to_string(),
                    default_model: "gpt-5.3-codex".to_string(),
                    connect_timeout_seconds: 10,
                    timeout_seconds: 120,
                    max_retries: 1,
                    retry_backoff_ms: 500,
                    max_output_tokens: 4096,
                    allow_untrusted_provider_url: false,
                },
            },
        }
    }
}
