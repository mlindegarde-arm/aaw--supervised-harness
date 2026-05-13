use crate::{HarnessError, HarnessResult};
use std::collections::BTreeMap;
use std::path::{Component, Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionResult {
    pub text: String,
    pub high_confidence_secret_detected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretConfidence {
    High,
    Medium,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    AuthHeader,
    ApiKey,
    CloudCredential,
    Cookie,
    HighEntropyToken,
    Password,
    PrivateKeyBlock,
    SessionToken,
    SshKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretFinding {
    pub kind: SecretKind,
    pub confidence: SecretConfidence,
}

pub trait Redactor {
    fn redact(&self, input: &str) -> RedactionResult;
    fn ensure_safe_for_escalation(&self, input: &str) -> HarnessResult<RedactionResult>;
}

pub trait EnvironmentSanitizer {
    fn sanitize(&self, env: &BTreeMap<String, String>) -> BTreeMap<String, String>;
}

pub trait ProviderUrlPolicy {
    fn validate_credentialed_url(&self, url: &str, allow_untrusted: bool) -> HarnessResult<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultRedactor;

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultEnvironmentSanitizer;

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultProviderUrlPolicy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionCheck {
    Secure,
    Warning(String),
    Failure(String),
}

impl DefaultRedactor {
    pub fn new() -> Self {
        Self
    }

    pub fn detect(&self, input: &str) -> Vec<SecretFinding> {
        let mut findings = Vec::new();
        let mut in_private_key = false;

        for line in input.lines() {
            let trimmed = line.trim();
            let lower = trimmed.to_ascii_lowercase();

            if is_private_key_begin(trimmed) {
                findings.push(high(SecretKind::PrivateKeyBlock));
                in_private_key = true;
                continue;
            }
            if in_private_key {
                if is_private_key_end(trimmed) {
                    in_private_key = false;
                }
                continue;
            }

            if is_sensitive_header(&lower, "authorization")
                || is_sensitive_header(&lower, "proxy-authorization")
            {
                findings.push(high(SecretKind::AuthHeader));
            }
            if is_sensitive_header(&lower, "cookie") || is_sensitive_header(&lower, "set-cookie") {
                findings.push(high(SecretKind::Cookie));
            }
            if looks_like_ssh_key(trimmed) {
                findings.push(high(SecretKind::SshKey));
            }
            if let Some((key, value)) = split_assignment_like(trimmed) {
                if !value.trim().is_empty() && is_secret_key_name(key) {
                    findings.push(high(secret_kind_for_key(key)));
                }
            }

            for token in token_candidates(trimmed) {
                if is_known_secret_token(token) {
                    findings.push(high(secret_kind_for_token(token)));
                } else if is_high_entropy_token(token) {
                    findings.push(high(SecretKind::HighEntropyToken));
                }
            }
        }

        findings
    }
}

impl Redactor for DefaultRedactor {
    fn redact(&self, input: &str) -> RedactionResult {
        let mut high_confidence_secret_detected = false;
        let mut output = String::with_capacity(input.len());
        let mut in_private_key = false;
        let mut private_key_replaced = false;

        for segment in input.split_inclusive('\n') {
            let (line, newline) = segment
                .strip_suffix('\n')
                .map_or((segment, ""), |line| (line, "\n"));
            let trimmed = line.trim();

            if is_private_key_begin(trimmed) {
                high_confidence_secret_detected = true;
                in_private_key = true;
                private_key_replaced = true;
                output.push_str("[REDACTED PRIVATE KEY BLOCK]");
                output.push_str(newline);
                continue;
            }
            if in_private_key {
                if is_private_key_end(trimmed) {
                    in_private_key = false;
                }
                if newline.is_empty() && !private_key_replaced {
                    output.push_str("[REDACTED PRIVATE KEY BLOCK]");
                }
                continue;
            }

            let (redacted_line, high_confidence) = redact_line(line);
            high_confidence_secret_detected |= high_confidence;
            output.push_str(&redacted_line);
            output.push_str(newline);
        }

        RedactionResult {
            text: output,
            high_confidence_secret_detected,
        }
    }

    fn ensure_safe_for_escalation(&self, input: &str) -> HarnessResult<RedactionResult> {
        let result = self.redact(input);
        if result.high_confidence_secret_detected {
            return Err(HarnessError::SecurityPolicy(
                "high-confidence secret detected in escalation evidence".to_string(),
            ));
        }
        Ok(result)
    }
}

impl DefaultEnvironmentSanitizer {
    pub fn new() -> Self {
        Self
    }
}

impl EnvironmentSanitizer for DefaultEnvironmentSanitizer {
    fn sanitize(&self, env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        env.iter()
            .filter_map(|(key, value)| {
                if is_allowed_env_name(key) {
                    Some((key.clone(), value.clone()))
                } else {
                    None
                }
            })
            .collect()
    }
}

impl DefaultProviderUrlPolicy {
    pub fn new() -> Self {
        Self
    }
}

impl ProviderUrlPolicy for DefaultProviderUrlPolicy {
    fn validate_credentialed_url(&self, url: &str, allow_untrusted: bool) -> HarnessResult<()> {
        let parsed = ParsedUrl::parse(url)?;

        if parsed.has_query_or_fragment {
            return Err(HarnessError::SecurityPolicy(
                "credentialed provider URL must not contain query or fragment".to_string(),
            ));
        }

        if provider_url_contains_secret_material(url) {
            return Err(HarnessError::SecurityPolicy(
                "credentialed provider URL must not contain API-key-like data".to_string(),
            ));
        }

        if parsed.scheme == "https" {
            if parsed.host == "openai-api-proxy.geo.arm.com" {
                return Ok(());
            }
            return Err(HarnessError::SecurityPolicy(format!(
                "credentialed provider host {:?} is not allowed",
                parsed.host
            )));
        }

        if parsed.scheme == "http" && allow_untrusted && is_localhost(&parsed.host) {
            return Ok(());
        }

        if parsed.scheme == "http" {
            return Err(HarnessError::SecurityPolicy(
                "credentialed provider URLs must use HTTPS unless explicitly allowing localhost test fakes"
                    .to_string(),
            ));
        }

        Err(HarnessError::SecurityPolicy(format!(
            "unsupported provider URL scheme {:?}",
            parsed.scheme
        )))
    }
}

pub fn should_exclude_context_path(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    path.components().any(is_sensitive_component)
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_sensitive_file_name)
}

pub fn is_path_within_root(root: impl AsRef<Path>, candidate: impl AsRef<Path>) -> bool {
    let root = normalize_components(root.as_ref());
    let candidate = normalize_components(candidate.as_ref());
    !root.is_empty() && candidate.starts_with(&root)
}

#[cfg(unix)]
pub fn check_private_permissions(
    path: impl AsRef<Path>,
    must_be_directory: bool,
) -> PermissionCheck {
    use std::os::unix::fs::PermissionsExt;

    let path = path.as_ref();
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return PermissionCheck::Failure(format!(
                "unable to read permissions for {}: {error}",
                path.display()
            ));
        }
    };

    if must_be_directory && !metadata.is_dir() {
        return PermissionCheck::Failure(format!("{} is not a directory", path.display()));
    }

    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return PermissionCheck::Warning(format!(
            "{} is group/world accessible ({mode:o})",
            path.display()
        ));
    }

    PermissionCheck::Secure
}

#[cfg(not(unix))]
pub fn check_private_permissions(
    _path: impl AsRef<Path>,
    _must_be_directory: bool,
) -> PermissionCheck {
    PermissionCheck::Warning("permission checks are not supported on this platform".to_string())
}

fn redact_line(line: &str) -> (String, bool) {
    let lower = line.trim_start().to_ascii_lowercase();
    if let Some(redacted) = redact_sensitive_header(line, &lower, "authorization") {
        return (redacted, true);
    }
    if let Some(redacted) = redact_sensitive_header(line, &lower, "proxy-authorization") {
        return (redacted, true);
    }
    if let Some(redacted) = redact_sensitive_header(line, &lower, "cookie") {
        return (redacted, true);
    }
    if let Some(redacted) = redact_sensitive_header(line, &lower, "set-cookie") {
        return (redacted, true);
    }
    if let Some(redacted) = redact_embedded_auth_header(line) {
        return (redacted, true);
    }
    if looks_like_ssh_key(line.trim()) {
        return ("[REDACTED SSH KEY]".to_string(), true);
    }

    let mut redacted = line.to_string();
    let mut high_confidence = false;

    if let Some((key, value)) = split_assignment_like(line.trim()) {
        if !value.trim().is_empty() && is_secret_key_name(key) {
            redacted = redact_assignment_value(line);
            high_confidence = true;
        }
    }

    let mut tokens: Vec<String> = token_candidates(&redacted)
        .into_iter()
        .map(str::to_string)
        .collect();
    tokens.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    tokens.dedup();

    for token in tokens {
        if looks_like_ssh_key(&token) {
            redacted = redacted.replace(&token, "[REDACTED SSH KEY]");
            high_confidence = true;
        } else if is_known_secret_token(&token) || is_high_entropy_token(&token) {
            redacted = redacted.replace(&token, "[REDACTED SECRET]");
            high_confidence = true;
        }
    }

    (redacted, high_confidence)
}

fn redact_sensitive_header(line: &str, lower_trimmed: &str, name: &str) -> Option<String> {
    if !is_sensitive_header(lower_trimmed, name) {
        return None;
    }

    let colon = line.find(':')?;
    Some(format!("{}: [REDACTED]", &line[..colon]))
}

fn redact_embedded_auth_header(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    for needle in [
        "authorization:",
        "proxy-authorization:",
        "cookie:",
        "set-cookie:",
    ] {
        if let Some(index) = lower.find(needle) {
            let prefix = &line[..index + needle.len()];
            return Some(format!("{prefix} [REDACTED]"));
        }
    }

    None
}

fn redact_assignment_value(line: &str) -> String {
    let Some(separator_index) = find_assignment_separator(line) else {
        return line.to_string();
    };
    let (before_separator, after_separator) = line.split_at(separator_index + 1);
    let whitespace_len = after_separator
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map_or(after_separator.len(), |(index, _)| index);
    let whitespace = &after_separator[..whitespace_len];
    let leading_quote = after_separator[whitespace_len..]
        .chars()
        .next()
        .filter(|ch| *ch == '"' || *ch == '\'');

    match leading_quote {
        Some(quote) => format!("{before_separator}{whitespace}{quote}[REDACTED]{quote}"),
        None => format!("{before_separator}{whitespace}[REDACTED]"),
    }
}

fn split_assignment_like(line: &str) -> Option<(&str, &str)> {
    let index = find_assignment_separator(line)?;
    let (raw_key, raw_value) = line.split_at(index);
    let key = raw_key
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_start_matches("export ")
        .trim();
    let value = raw_value[1..].trim();

    if key.is_empty() || value.is_empty() || key.contains(' ') && !key.contains('-') {
        return None;
    }

    Some((key, value))
}

fn find_assignment_separator(line: &str) -> Option<usize> {
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    for (index, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double_quote => in_single_quote = !in_single_quote,
            '"' if !in_single_quote => in_double_quote = !in_double_quote,
            '=' | ':' if !in_single_quote && !in_double_quote => return Some(index),
            _ => {}
        }
    }

    None
}

fn token_candidates(line: &str) -> Vec<&str> {
    line.split(|ch: char| {
        !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+' | '/' | '=' | '.'))
    })
    .map(|token| token.trim_matches(|ch| matches!(ch, '"' | '\'' | ',' | ';' | ')' | '(')))
    .filter(|token| token.len() >= 20)
    .collect()
}

fn is_sensitive_header(lower_trimmed: &str, name: &str) -> bool {
    lower_trimmed
        .strip_prefix(name)
        .is_some_and(|rest| rest.trim_start().starts_with(':'))
}

fn is_secret_key_name(key: &str) -> bool {
    let normalized = key
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_start_matches("export ")
        .to_ascii_lowercase();

    [
        "api_key",
        "apikey",
        "auth",
        "authorization",
        "aws_access_key_id",
        "aws_secret_access_key",
        "azure_client_secret",
        "cookie",
        "credential",
        "github_token",
        "openai_api_key",
        "password",
        "private_key",
        "proxy_token",
        "refresh_token",
        "secret",
        "session",
        "token",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn is_allowed_env_name(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CI" | "COLUMNS"
            | "HOME"
            | "LANG"
            | "LC_ALL"
            | "LINES"
            | "NO_COLOR"
            | "PATH"
            | "PWD"
            | "RUST_BACKTRACE"
            | "RUST_LOG"
            | "TERM"
            | "TMPDIR"
    )
}

fn secret_kind_for_key(key: &str) -> SecretKind {
    let lower = key.to_ascii_lowercase();
    if lower.contains("password") {
        SecretKind::Password
    } else if lower.contains("cookie") {
        SecretKind::Cookie
    } else if lower.contains("session") || lower.contains("token") {
        SecretKind::SessionToken
    } else if lower.contains("aws") || lower.contains("azure") || lower.contains("google") {
        SecretKind::CloudCredential
    } else {
        SecretKind::ApiKey
    }
}

fn is_private_key_begin(line: &str) -> bool {
    line.starts_with("-----BEGIN ") && line.ends_with(" PRIVATE KEY-----")
}

fn is_private_key_end(line: &str) -> bool {
    line.starts_with("-----END ") && line.ends_with(" PRIVATE KEY-----")
}

fn looks_like_ssh_key(value: &str) -> bool {
    let trimmed = value.trim();
    ["ssh-rsa ", "ssh-ed25519 ", "ecdsa-sha2-nistp256 "]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
        || ["ssh-rsa-", "ssh-ed25519-", "ecdsa-sha2-nistp256-"]
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
}

fn is_known_secret_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    token.starts_with("sk-")
        || token.starts_with("ghp_")
        || token.starts_with("gho_")
        || token.starts_with("github_pat_")
        || token.starts_with("xoxb-")
        || token.starts_with("xoxp-")
        || token.starts_with("AKIA")
        || token.starts_with("ASIA")
        || token.starts_with("AIza")
        || lower.starts_with("bearer.")
}

fn secret_kind_for_token(token: &str) -> SecretKind {
    if token.starts_with("AKIA") || token.starts_with("ASIA") || token.starts_with("AIza") {
        SecretKind::CloudCredential
    } else if token.starts_with("gh")
        || token.starts_with("github")
        || token.starts_with("xox")
        || token.starts_with("sk-")
    {
        SecretKind::ApiKey
    } else {
        SecretKind::HighEntropyToken
    }
}

fn is_high_entropy_token(token: &str) -> bool {
    if token.len() < 32
        || token.contains('.')
        || token.contains('/')
        || token.chars().all(|ch| ch.is_ascii_digit())
    {
        return false;
    }

    let has_lower = token.chars().any(|ch| ch.is_ascii_lowercase());
    let has_upper = token.chars().any(|ch| ch.is_ascii_uppercase());
    let has_digit = token.chars().any(|ch| ch.is_ascii_digit());
    let has_symbol = token.chars().any(|ch| matches!(ch, '_' | '-' | '+' | '='));
    let classes = [has_lower, has_upper, has_digit, has_symbol]
        .iter()
        .filter(|present| **present)
        .count();

    classes >= 3 && shannon_entropy(token) >= 4.0
}

fn shannon_entropy(value: &str) -> f64 {
    let mut counts = BTreeMap::new();
    for byte in value.bytes() {
        *counts.entry(byte).or_insert(0usize) += 1;
    }

    let len = value.len() as f64;
    counts
        .values()
        .map(|count| {
            let probability = *count as f64 / len;
            -probability * probability.log2()
        })
        .sum()
}

fn high(kind: SecretKind) -> SecretFinding {
    SecretFinding {
        kind,
        confidence: SecretConfidence::High,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedUrl {
    scheme: String,
    host: String,
    has_query_or_fragment: bool,
}

impl ParsedUrl {
    fn parse(url: &str) -> HarnessResult<Self> {
        let (scheme, rest) = url.split_once("://").ok_or_else(|| {
            HarnessError::SecurityPolicy("provider URL is missing scheme".to_string())
        })?;
        let has_query_or_fragment = rest.contains('?') || rest.contains('#');
        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .filter(|authority| !authority.is_empty())
            .ok_or_else(|| {
                HarnessError::SecurityPolicy("provider URL is missing host".to_string())
            })?;

        if authority.contains('@') {
            return Err(HarnessError::SecurityPolicy(
                "provider URL must not contain userinfo".to_string(),
            ));
        }

        let host = parse_authority_host(authority)?;
        Ok(Self {
            scheme: scheme.to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            has_query_or_fragment,
        })
    }
}

fn parse_authority_host(authority: &str) -> HarnessResult<String> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, _) = rest.split_once(']').ok_or_else(|| {
            HarnessError::SecurityPolicy("invalid IPv6 provider host".to_string())
        })?;
        return Ok(host.to_string());
    }

    Ok(authority
        .split(':')
        .next()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| HarnessError::SecurityPolicy("provider URL is missing host".to_string()))?
        .to_string())
}

fn is_localhost(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn provider_url_contains_secret_material(url: &str) -> bool {
    provider_url_secret_candidates(url)
        .into_iter()
        .any(|token| is_known_secret_token(token) || is_high_entropy_token(token))
}

fn provider_url_secret_candidates(url: &str) -> Vec<&str> {
    url.split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+' | '=')))
        .filter(|token| token.len() >= 20)
        .collect()
}

fn is_sensitive_component(component: Component<'_>) -> bool {
    match component {
        Component::Normal(value) => value.to_str().is_some_and(is_sensitive_file_name),
        _ => false,
    }
}

fn is_sensitive_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with(".env")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower == ".netrc"
        || lower == "credentials"
        || lower == "credentials.json"
        || lower == "id_rsa"
        || lower == "id_dsa"
        || lower == "id_ecdsa"
        || lower == "id_ed25519"
        || lower == "config.toml"
        || lower == ".npmrc"
        || lower == ".pypirc"
        || lower == ".ssh"
        || lower == ".docker"
        || lower == ".kube"
        || lower == ".aws"
        || lower == ".config"
}

fn normalize_components(path: &Path) -> Vec<String> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                components.push(prefix.as_os_str().to_string_lossy().into())
            }
            Component::RootDir => components.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                components.pop();
            }
            Component::Normal(value) => components.push(value.to_string_lossy().into()),
        }
    }
    components
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn redacts_auth_cookie_and_assignment_secrets() {
        let input = concat!(
            "Authorization: Bearer sk-test_abcdefghijklmnopqrstuvwxyz123456\n",
            "Proxy-Authorization: Basic dXNlcjpzdXBlcnNlY3JldA==\n",
            "Cookie: sessionid=abcdef1234567890\n",
            "OPENAI_API_KEY=sk-proj-abcdefABCDEF1234567890abcdefABCDEF\n",
            "password: hunter2\n",
            "\"session_token\": \"abcdefABCDEF1234567890abcdefABCDEF123456\"\n",
        );

        let result = DefaultRedactor.redact(input);

        assert!(result.high_confidence_secret_detected);
        assert!(result.text.contains("Authorization: [REDACTED]"));
        assert!(result.text.contains("Proxy-Authorization: [REDACTED]"));
        assert!(result.text.contains("Cookie: [REDACTED]"));
        assert!(result.text.contains("OPENAI_API_KEY=[REDACTED]"));
        assert!(result.text.contains("password: [REDACTED]"));
        assert!(result.text.contains("\"session_token\": \"[REDACTED]\""));
        assert!(!result.text.contains("hunter2"));
        assert!(!result.text.contains("sk-proj-abcdef"));
    }

    #[test]
    fn redacts_private_and_ssh_keys() {
        let input = concat!(
            "before\n",
            "-----BEGIN OPENSSH PRIVATE KEY-----\n",
            "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAA=\n",
            "-----END OPENSSH PRIVATE KEY-----\n",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEHXabcdefghijklmnopqrstuvwxyz1234567890 comment\n",
            "after\n",
        );

        let result = DefaultRedactor.redact(input);

        assert!(result.high_confidence_secret_detected);
        assert!(result.text.contains("[REDACTED PRIVATE KEY BLOCK]"));
        assert!(result.text.contains("[REDACTED SSH KEY]"));
        assert!(!result.text.contains("b3BlbnNzaC1rZXktdjE"));
        assert!(!result.text.contains("AAAAC3NzaC1lZDI1NTE5"));
        assert!(result.text.contains("after"));
    }

    #[test]
    fn redacts_cloud_credentials_and_high_entropy_tokens() {
        let input = concat!(
            "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\n",
            "AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n",
            "token=AbCdEfGhIjKlMnOpQrStUvWxYz1234567890_+-=\n",
        );

        let result = DefaultRedactor.redact(input);

        assert!(result.high_confidence_secret_detected);
        assert!(!result.text.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!result.text.contains("wJalrXUtnFEMI"));
        assert!(!result.text.contains("AbCdEfGhIjKl"));
        assert_eq!(result.text.matches("[REDACTED]").count(), 3);
    }

    #[test]
    fn escalation_blocks_when_high_confidence_secret_is_seen() {
        let error = DefaultRedactor
            .ensure_safe_for_escalation("provider failed with Authorization: Bearer secret")
            .unwrap_err();

        assert!(
            matches!(error, HarnessError::SecurityPolicy(message) if message.contains("high-confidence secret"))
        );
    }

    #[test]
    fn escalation_allows_non_secret_evidence() {
        let result = DefaultRedactor
            .ensure_safe_for_escalation("validation failed: expected 4, got 5")
            .unwrap();

        assert_eq!(result.text, "validation failed: expected 4, got 5");
        assert!(!result.high_confidence_secret_detected);
    }

    #[test]
    fn sanitizer_keeps_only_allowlisted_environment_variables() {
        let env = BTreeMap::from([
            ("PATH".to_string(), "/bin".to_string()),
            ("HOME".to_string(), "/tmp/home".to_string()),
            ("ARM_OPENAI_API_KEY".to_string(), "secret".to_string()),
            ("AWS_SESSION_TOKEN".to_string(), "session".to_string()),
            ("DATABASE_PASSWORD".to_string(), "password".to_string()),
            ("SSH_AUTH_SOCK".to_string(), "/tmp/agent.sock".to_string()),
            ("HARMLESS_FEATURE_FLAG".to_string(), "1".to_string()),
            ("RUST_LOG".to_string(), "info".to_string()),
        ]);

        let sanitized = DefaultEnvironmentSanitizer.sanitize(&env);

        assert_eq!(
            sanitized.keys().cloned().collect::<Vec<_>>(),
            vec!["HOME", "PATH", "RUST_LOG"]
        );
        assert_eq!(sanitized.get("PATH"), Some(&"/bin".to_string()));
        assert_eq!(sanitized.get("HOME"), Some(&"/tmp/home".to_string()));
        assert_eq!(sanitized.get("RUST_LOG"), Some(&"info".to_string()));
        assert!(!sanitized.contains_key("ARM_OPENAI_API_KEY"));
        assert!(!sanitized.contains_key("AWS_SESSION_TOKEN"));
        assert!(!sanitized.contains_key("DATABASE_PASSWORD"));
        assert!(!sanitized.contains_key("SSH_AUTH_SOCK"));
        assert!(!sanitized.contains_key("HARMLESS_FEATURE_FLAG"));
    }

    #[test]
    fn provider_url_policy_allows_only_arm_https_by_default() {
        let policy = DefaultProviderUrlPolicy;

        assert!(
            policy
                .validate_credentialed_url(
                    "https://openai-api-proxy.geo.arm.com/api/providers/openai-us/v1",
                    false,
                )
                .is_ok()
        );
        assert!(
            policy
                .validate_credentialed_url("https://api.openai.com/v1", false)
                .is_err()
        );
        assert!(
            policy
                .validate_credentialed_url("http://openai-api-proxy.geo.arm.com/v1", false)
                .is_err()
        );
    }

    #[test]
    fn provider_url_policy_rejects_query_fragment_and_key_like_path_data() {
        let policy = DefaultProviderUrlPolicy;

        for url in [
            "https://openai-api-proxy.geo.arm.com/api/providers/openai-us/v1?api_key=sk-test_abcdefghijklmnopqrstuvwxyz123456",
            "https://openai-api-proxy.geo.arm.com/api/providers/openai-us/v1#sk-test_abcdefghijklmnopqrstuvwxyz123456",
            "https://openai-api-proxy.geo.arm.com/api/providers/openai-us/sk-test_abcdefghijklmnopqrstuvwxyz123456",
            "https://openai-api-proxy.geo.arm.com/api/providers/openai-us/AbCdEfGhIjKlMnOpQrStUvWxYz1234567890_+-=",
        ] {
            assert!(
                policy.validate_credentialed_url(url, false).is_err(),
                "{url} should be rejected"
            );
        }
    }

    #[test]
    fn provider_url_policy_allows_http_localhost_only_when_untrusted_is_enabled() {
        let policy = DefaultProviderUrlPolicy;

        assert!(
            policy
                .validate_credentialed_url("http://localhost:8080/v1", true)
                .is_ok()
        );
        assert!(
            policy
                .validate_credentialed_url("http://127.0.0.1:8080/v1", true)
                .is_ok()
        );
        assert!(
            policy
                .validate_credentialed_url("http://[::1]:8080/v1", true)
                .is_ok()
        );
        assert!(
            policy
                .validate_credentialed_url("http://localhost:8080/v1", false)
                .is_err()
        );
        assert!(
            policy
                .validate_credentialed_url("http://example.com/v1", true)
                .is_err()
        );
    }

    #[test]
    fn context_path_exclusions_cover_env_keys_credentials_and_local_config() {
        assert!(should_exclude_context_path(".env"));
        assert!(should_exclude_context_path(".env.local"));
        assert!(should_exclude_context_path(".envrc"));
        assert!(should_exclude_context_path("keys/id_ed25519"));
        assert!(should_exclude_context_path(".ssh/id_ed25519.pub"));
        assert!(should_exclude_context_path("home/user/.ssh/known_hosts"));
        assert!(should_exclude_context_path("secrets/client.pem"));
        assert!(should_exclude_context_path(".aws/credentials"));
        assert!(should_exclude_context_path(".harness/config.toml"));
        assert!(!should_exclude_context_path("src/main.rs"));
    }

    #[test]
    fn path_containment_rejects_parent_escape() {
        assert!(is_path_within_root(
            "/repo/.harness",
            "/repo/.harness/artifacts/a.txt"
        ));
        assert!(!is_path_within_root(
            "/repo/.harness",
            "/repo/.harness/../.env"
        ));
        assert!(!is_path_within_root(
            "/repo/.harness",
            "/repo-other/.harness/a.txt"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn permission_check_warns_on_group_or_world_access() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let path = temp.path().join("artifact.txt");
        std::fs::write(&path, "redacted").unwrap();

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            check_private_permissions(&path, false),
            PermissionCheck::Secure
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            check_private_permissions(&path, false),
            PermissionCheck::Warning(message) if message.contains("group/world accessible")
        ));
    }
}
