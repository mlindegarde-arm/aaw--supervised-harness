use crate::security::RepoPath;
use crate::{HarnessError, HarnessResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const PLANNER_SCHEMA_VERSION: u32 = 1;
pub const RESOLVER_SCHEMA_VERSION: u32 = 1;
pub const CONTEXT_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const MAX_PLANNER_RESPONSE_BYTES: usize = 256 * 1024;
pub const MAX_PLANNER_TASKS: usize = 64;
pub const MAX_ACCEPTANCE_CRITERIA: usize = 64;
pub const MAX_VALIDATION_COMMANDS: usize = 32;
pub const MAX_TASK_VALIDATION_COMMANDS: usize = 16;
pub const MAX_FINAL_VERIFICATION_STEPS: usize = 32;
pub const MAX_RISKS: usize = 32;
pub const MAX_STRING_BYTES: usize = 8 * 1024;
pub const MAX_TASK_KEY_BYTES: usize = 96;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerRequest {
    pub schema_version: u32,
    pub objective_prompt: String,
    pub repository_context: String,
    pub context_manifest: ContextManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerResponse {
    pub schema_version: u32,
    pub objective: PlannerObjective,
    pub tasks: Vec<PlannerTask>,
    pub risks: Vec<String>,
    pub final_verification: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerObjective {
    pub title: String,
    pub summary: String,
    pub acceptance_criteria: Vec<String>,
    pub validation_commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerTask {
    pub task_key: String,
    pub title: String,
    pub goal: String,
    pub validation: Vec<String>,
    pub depends_on: Vec<String>,
    pub owned_paths: Vec<String>,
    pub parallel_group: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TicketResolverRequest {
    pub schema_version: u32,
    pub objective_prompt: String,
    pub objective_summary: String,
    pub acceptance_criteria: Vec<String>,
    pub objective_status: String,
    pub task_status: String,
    pub ticket_status: String,
    pub ticket_details: TicketResolverTicketDetails,
    pub task_details: TicketResolverTaskDetails,
    pub prior_resolutions: Vec<String>,
    pub context_manifest: ContextManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TicketResolverTicketDetails {
    pub blocked_on: String,
    pub reason: String,
    pub question: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TicketResolverTaskDetails {
    pub title: String,
    pub goal: String,
    pub validation_commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TicketResolverResponse {
    pub schema_version: u32,
    pub diagnosis: String,
    pub recommended_steps: Vec<String>,
    pub constraints: Vec<String>,
    pub validation_focus: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBudget {
    pub total_bytes: usize,
    pub objective_bytes: usize,
    pub conversation_bytes: usize,
    pub state_bytes: usize,
    pub artifact_excerpt_bytes: usize,
    pub schema_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSection {
    pub label: String,
    pub priority: u32,
    pub budget_class: ContextBudgetClass,
    pub required: bool,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContextBudgetClass {
    Objective,
    Conversation,
    State,
    ArtifactExcerpt,
    Schema,
}

impl ContextBudgetClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Objective => "objective",
            Self::Conversation => "conversation",
            Self::State => "state",
            Self::ArtifactExcerpt => "artifact_excerpt",
            Self::Schema => "schema",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextArtifact {
    pub artifact_id: String,
    pub label: String,
    pub priority: u32,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPackRequest {
    pub budget: ContextBudget,
    pub sections: Vec<ContextSection>,
    pub artifacts: Vec<ContextArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedContext {
    pub body: String,
    pub manifest: ContextManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextManifest {
    pub schema_version: u32,
    pub budget: ContextBudget,
    pub included_sections: Vec<ContextSectionManifest>,
    pub omitted_sections: Vec<ContextOmittedSection>,
    pub artifacts: Vec<ContextArtifactManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSectionManifest {
    pub label: String,
    pub budget_class: String,
    pub original_bytes: usize,
    pub included_bytes: usize,
    pub truncated: bool,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextOmittedSection {
    pub label: String,
    pub budget_class: String,
    pub original_bytes: usize,
    pub reason: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextArtifactManifest {
    pub artifact_id: String,
    pub label: String,
    pub original_bytes: usize,
    pub included_bytes: usize,
    pub truncated: bool,
    pub sha256: String,
}

pub fn parse_planner_response(raw: &str) -> HarnessResult<PlannerResponse> {
    reject_wrapped_json(raw, MAX_PLANNER_RESPONSE_BYTES, "planner")?;
    let response: PlannerResponse = serde_json::from_str(raw)
        .map_err(|error| schema_error(format!("invalid planner JSON: {error}")))?;
    validate_planner_response(&response)?;
    Ok(response)
}

pub fn parse_planner_response_for_repo(
    raw: &str,
    repo_root: impl AsRef<Path>,
) -> HarnessResult<PlannerResponse> {
    let response = parse_planner_response(raw)?;
    validate_planner_response_for_repo(&response, repo_root)?;
    Ok(response)
}

pub fn parse_ticket_resolver_response(raw: &str) -> HarnessResult<TicketResolverResponse> {
    reject_wrapped_json(raw, MAX_PLANNER_RESPONSE_BYTES, "resolver")?;
    let response: TicketResolverResponse = serde_json::from_str(raw)
        .map_err(|error| schema_error(format!("invalid resolver JSON: {error}")))?;
    validate_ticket_resolver_response(&response)?;
    Ok(response)
}

pub fn build_planner_request(
    objective_prompt: impl Into<String>,
    repository_context: impl Into<String>,
    context_manifest: ContextManifest,
) -> PlannerRequest {
    PlannerRequest {
        schema_version: PLANNER_SCHEMA_VERSION,
        objective_prompt: objective_prompt.into(),
        repository_context: repository_context.into(),
        context_manifest,
    }
}

pub fn build_ticket_resolver_request(
    objective_prompt: impl Into<String>,
    objective_summary: impl Into<String>,
    acceptance_criteria: Vec<String>,
    statuses: TicketResolverStatuses,
    ticket_details: TicketResolverTicketDetails,
    task_details: TicketResolverTaskDetails,
    prior_resolutions: Vec<String>,
    context_manifest: ContextManifest,
) -> TicketResolverRequest {
    TicketResolverRequest {
        schema_version: RESOLVER_SCHEMA_VERSION,
        objective_prompt: objective_prompt.into(),
        objective_summary: objective_summary.into(),
        acceptance_criteria,
        objective_status: statuses.objective_status,
        task_status: statuses.task_status,
        ticket_status: statuses.ticket_status,
        ticket_details,
        task_details,
        prior_resolutions,
        context_manifest,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketResolverStatuses {
    pub objective_status: String,
    pub task_status: String,
    pub ticket_status: String,
}

pub fn pack_context(request: ContextPackRequest) -> HarnessResult<PackedContext> {
    validate_budget(&request.budget)?;

    let mut class_remaining = BTreeMap::from([
        (
            ContextBudgetClass::Objective,
            request.budget.objective_bytes,
        ),
        (
            ContextBudgetClass::Conversation,
            request.budget.conversation_bytes,
        ),
        (ContextBudgetClass::State, request.budget.state_bytes),
        (
            ContextBudgetClass::ArtifactExcerpt,
            request.budget.artifact_excerpt_bytes,
        ),
        (ContextBudgetClass::Schema, request.budget.schema_bytes),
    ]);
    let mut total_remaining = request.budget.total_bytes;
    let mut body_parts = Vec::new();
    let mut included_sections = Vec::new();
    let mut omitted_sections = Vec::new();

    let mut sections = request.sections;
    sections.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.budget_class.cmp(&right.budget_class))
            .then_with(|| left.label.cmp(&right.label))
    });

    for section in sections {
        let redacted = redact_secret_like_values(&section.body);
        let original_bytes = redacted.len();
        let class_budget = *class_remaining.get(&section.budget_class).unwrap_or(&0);
        let available = class_budget.min(total_remaining);
        let sha256 = sha256_hex(redacted.as_bytes());

        if available == 0 {
            if section.required {
                return Err(schema_error(format!(
                    "required context section {} has no remaining budget",
                    section.label
                )));
            }
            omitted_sections.push(ContextOmittedSection {
                label: section.label,
                budget_class: section.budget_class.as_str().to_string(),
                original_bytes,
                reason: "budget_exhausted".to_string(),
                sha256,
            });
            continue;
        }

        let (included, truncated) = truncate_with_label(&redacted, available);
        let included_bytes = included.len();
        if section.required && truncated {
            return Err(schema_error(format!(
                "required context section {} exceeds budget",
                section.label
            )));
        }
        if included_bytes == 0 {
            omitted_sections.push(ContextOmittedSection {
                label: section.label,
                budget_class: section.budget_class.as_str().to_string(),
                original_bytes,
                reason: "budget_exhausted".to_string(),
                sha256,
            });
            continue;
        }

        let rendered = render_context_section(
            &section.label,
            original_bytes,
            included_bytes,
            truncated,
            &included,
        );
        total_remaining = total_remaining.saturating_sub(included_bytes);
        if let Some(remaining) = class_remaining.get_mut(&section.budget_class) {
            *remaining = remaining.saturating_sub(included_bytes);
        }
        body_parts.push(rendered);
        included_sections.push(ContextSectionManifest {
            label: section.label,
            budget_class: section.budget_class.as_str().to_string(),
            original_bytes,
            included_bytes,
            truncated,
            sha256,
        });
    }

    let mut artifacts = request.artifacts;
    artifacts.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.artifact_id.cmp(&right.artifact_id))
            .then_with(|| left.label.cmp(&right.label))
    });

    let mut artifact_manifest = Vec::new();
    for artifact in artifacts {
        let redacted = redact_secret_like_values(&artifact.body);
        let original_bytes = redacted.len();
        let available = class_remaining
            .get(&ContextBudgetClass::ArtifactExcerpt)
            .copied()
            .unwrap_or(0)
            .min(total_remaining);
        let (included, truncated) = truncate_with_label(&redacted, available);
        let included_bytes = included.len();
        let sha256 = sha256_hex(redacted.as_bytes());
        if included_bytes > 0 {
            let label = format!("artifact:{}:{}", artifact.artifact_id, artifact.label);
            body_parts.push(render_context_section(
                &label,
                original_bytes,
                included_bytes,
                truncated,
                &included,
            ));
            total_remaining = total_remaining.saturating_sub(included_bytes);
            if let Some(remaining) = class_remaining.get_mut(&ContextBudgetClass::ArtifactExcerpt) {
                *remaining = remaining.saturating_sub(included_bytes);
            }
        }
        artifact_manifest.push(ContextArtifactManifest {
            artifact_id: artifact.artifact_id,
            label: artifact.label,
            original_bytes,
            included_bytes,
            truncated,
            sha256,
        });
    }

    Ok(PackedContext {
        body: body_parts.join("\n\n"),
        manifest: ContextManifest {
            schema_version: CONTEXT_MANIFEST_SCHEMA_VERSION,
            budget: request.budget,
            included_sections,
            omitted_sections,
            artifacts: artifact_manifest,
        },
    })
}

pub fn validate_planner_response(response: &PlannerResponse) -> HarnessResult<()> {
    if response.schema_version != PLANNER_SCHEMA_VERSION {
        return Err(schema_error(format!(
            "unsupported planner schema_version {}",
            response.schema_version
        )));
    }
    validate_non_empty_string("objective.title", &response.objective.title)?;
    validate_non_empty_string("objective.summary", &response.objective.summary)?;
    validate_string_list(
        "objective.acceptance_criteria",
        &response.objective.acceptance_criteria,
        1,
        MAX_ACCEPTANCE_CRITERIA,
    )?;
    validate_string_list(
        "objective.validation_commands",
        &response.objective.validation_commands,
        1,
        MAX_VALIDATION_COMMANDS,
    )?;
    validate_string_list("risks", &response.risks, 0, MAX_RISKS)?;
    validate_string_list(
        "final_verification",
        &response.final_verification,
        0,
        MAX_FINAL_VERIFICATION_STEPS,
    )?;

    if response.tasks.is_empty() {
        return Err(schema_error("planner returned no tasks"));
    }
    if response.tasks.len() > MAX_PLANNER_TASKS {
        return Err(schema_error(format!(
            "planner returned {} tasks; maximum is {}",
            response.tasks.len(),
            MAX_PLANNER_TASKS
        )));
    }

    let mut keys = BTreeSet::new();
    for task in &response.tasks {
        validate_task_key(&task.task_key)?;
        if !keys.insert(task.task_key.as_str()) {
            return Err(schema_error(format!(
                "duplicate planner task_key {}",
                task.task_key
            )));
        }
        validate_non_empty_string("task.title", &task.title)?;
        validate_non_empty_string("task.goal", &task.goal)?;
        validate_string_list(
            "task.validation",
            &task.validation,
            1,
            MAX_TASK_VALIDATION_COMMANDS,
        )?;
        validate_string_list("task.depends_on", &task.depends_on, 0, MAX_PLANNER_TASKS)?;
        validate_string_list("task.owned_paths", &task.owned_paths, 0, MAX_PLANNER_TASKS)?;
        validate_non_empty_string("task.parallel_group", &task.parallel_group)?;
        for path in &task.owned_paths {
            validate_repo_path(path)?;
        }
    }

    for task in &response.tasks {
        for dependency in &task.depends_on {
            if !keys.contains(dependency.as_str()) {
                return Err(schema_error(format!(
                    "task {} depends on unknown task_key {}",
                    task.task_key, dependency
                )));
            }
            if dependency == &task.task_key {
                return Err(schema_error(format!(
                    "task {} cannot depend on itself",
                    task.task_key
                )));
            }
        }
    }
    validate_acyclic(response)
}

pub fn validate_planner_response_for_repo(
    response: &PlannerResponse,
    repo_root: impl AsRef<Path>,
) -> HarnessResult<()> {
    validate_planner_response(response)?;
    let repo_root = repo_root.as_ref();
    for task in &response.tasks {
        for path in &task.owned_paths {
            RepoPath::validate(repo_root, path).map_err(|error| {
                schema_error(format!(
                    "owned_path {path:?} rejected by repo path policy: {error}"
                ))
            })?;
        }
    }
    Ok(())
}

pub fn validate_ticket_resolver_response(response: &TicketResolverResponse) -> HarnessResult<()> {
    if response.schema_version != RESOLVER_SCHEMA_VERSION {
        return Err(schema_error(format!(
            "unsupported resolver schema_version {}",
            response.schema_version
        )));
    }
    validate_non_empty_string("diagnosis", &response.diagnosis)?;
    validate_string_list("recommended_steps", &response.recommended_steps, 1, 32)?;
    validate_string_list("constraints", &response.constraints, 0, 32)?;
    validate_string_list("validation_focus", &response.validation_focus, 0, 32)?;

    for (label, text) in resolver_texts(response) {
        reject_resolver_unsafe_text(label, text)?;
    }
    Ok(())
}

fn validate_acyclic(response: &PlannerResponse) -> HarnessResult<()> {
    let graph = response
        .tasks
        .iter()
        .map(|task| (task.task_key.as_str(), task.depends_on.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for key in graph.keys().copied() {
        visit_task_key(key, &graph, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn visit_task_key<'a>(
    key: &'a str,
    graph: &BTreeMap<&'a str, &'a [String]>,
    visiting: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
) -> HarnessResult<()> {
    if visited.contains(key) {
        return Ok(());
    }
    if !visiting.insert(key) {
        return Err(schema_error(format!(
            "planner dependency graph contains a cycle at {key}"
        )));
    }
    for dependency in graph.get(key).copied().unwrap_or(&[]) {
        visit_task_key(dependency.as_str(), graph, visiting, visited)?;
    }
    visiting.remove(key);
    visited.insert(key);
    Ok(())
}

fn reject_wrapped_json(raw: &str, max_bytes: usize, kind: &str) -> HarnessResult<()> {
    if raw.len() > max_bytes {
        return Err(schema_error(format!(
            "{kind} response is {} bytes; maximum is {max_bytes}",
            raw.len()
        )));
    }
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return Err(schema_error(format!(
            "{kind} response must be exactly one JSON object with no markdown or prose wrapper"
        )));
    }
    if trimmed.contains("```") {
        return Err(schema_error(format!(
            "{kind} response must not contain markdown fences"
        )));
    }
    Ok(())
}

fn validate_task_key(value: &str) -> HarnessResult<()> {
    if value.is_empty() {
        return Err(schema_error("task_key must not be empty"));
    }
    if value.len() > MAX_TASK_KEY_BYTES {
        return Err(schema_error(format!("task_key {value} is too long")));
    }
    let valid = value
        .bytes()
        .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_'))
        && value
            .bytes()
            .next()
            .is_some_and(|byte| matches!(byte, b'a'..=b'z'))
        && value
            .bytes()
            .last()
            .is_some_and(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9'))
        && !value.contains("__");
    if valid {
        Ok(())
    } else {
        Err(schema_error(format!(
            "task_key {value} must be stable lowercase snake-case"
        )))
    }
}

fn validate_repo_path(value: &str) -> HarnessResult<()> {
    validate_non_empty_string("owned_path", value)?;
    RepoPath::validate_lexical(value)
        .map(|_| ())
        .map_err(|error| {
            schema_error(format!(
                "owned_path {value:?} rejected by repo path policy: {error}"
            ))
        })
}

fn validate_string_list(
    label: &str,
    values: &[String],
    min_len: usize,
    max_len: usize,
) -> HarnessResult<()> {
    if values.len() < min_len {
        return Err(schema_error(format!(
            "{label} contains {} entries; minimum is {min_len}",
            values.len()
        )));
    }
    if values.len() > max_len {
        return Err(schema_error(format!(
            "{label} contains {} entries; maximum is {max_len}",
            values.len()
        )));
    }
    for value in values {
        validate_non_empty_string(label, value)?;
    }
    Ok(())
}

fn validate_non_empty_string(label: &str, value: &str) -> HarnessResult<()> {
    if value.trim().is_empty() {
        return Err(schema_error(format!("{label} must not be empty")));
    }
    if value.len() > MAX_STRING_BYTES {
        return Err(schema_error(format!(
            "{label} is {} bytes; maximum is {MAX_STRING_BYTES}",
            value.len()
        )));
    }
    Ok(())
}

fn resolver_texts(response: &TicketResolverResponse) -> Vec<(&'static str, &str)> {
    let mut texts = vec![("diagnosis", response.diagnosis.as_str())];
    texts.extend(
        response
            .recommended_steps
            .iter()
            .map(|value| ("recommended_steps", value.as_str())),
    );
    texts.extend(
        response
            .constraints
            .iter()
            .map(|value| ("constraints", value.as_str())),
    );
    texts.extend(
        response
            .validation_focus
            .iter()
            .map(|value| ("validation_focus", value.as_str())),
    );
    texts
}

fn reject_resolver_unsafe_text(label: &str, text: &str) -> HarnessResult<()> {
    let lower = text.to_ascii_lowercase();
    let patch_like = lower.contains("diff --git ")
        || lower.contains("```diff")
        || lower.contains("```patch")
        || ((lower.starts_with("--- ") || lower.contains("\n--- "))
            && (lower.contains("\n+++ ") || lower.starts_with("+++ "))
            && (lower.contains("\n@@") || lower.starts_with("@@")));
    if patch_like {
        return Err(schema_error(format!(
            "resolver {label} contains patch-like or diff-like content"
        )));
    }

    let script_like = lower.contains("```sh")
        || lower.contains("```bash")
        || lower.contains("```shell")
        || lower.starts_with("#!")
        || lower.contains("\n#!")
        || text.lines().any(is_executable_guidance_line);
    if script_like {
        return Err(schema_error(format!(
            "resolver {label} contains shell script or executable shell guidance"
        )));
    }

    Ok(())
}

fn is_executable_guidance_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("$ ") {
        return true;
    }

    let Some(command) = strip_list_marker(trimmed) else {
        return false;
    };
    command_starts_with_executable(command)
}

fn strip_list_marker(trimmed: &str) -> Option<&str> {
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return Some(rest.trim_start());
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return Some(rest.trim_start());
    }
    if let Some(rest) = trimmed.strip_prefix("+ ") {
        return Some(rest.trim_start());
    }

    let marker_end = trimmed
        .char_indices()
        .find_map(|(index, ch)| (ch == '.' || ch == ')').then_some(index))?;
    if marker_end == 0 || !trimmed[..marker_end].chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(trimmed[marker_end + 1..].trim_start())
}

fn command_starts_with_executable(command: &str) -> bool {
    let first = command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|ch: char| matches!(ch, '`' | '"' | '\''));
    if first.is_empty() {
        return false;
    }
    if first.starts_with("./") || first.starts_with("../") || first.contains('/') {
        return true;
    }
    first
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn validate_budget(budget: &ContextBudget) -> HarnessResult<()> {
    let subtotal = budget.objective_bytes
        + budget.conversation_bytes
        + budget.state_bytes
        + budget.artifact_excerpt_bytes
        + budget.schema_bytes;
    if budget.total_bytes == 0 {
        return Err(schema_error("context total_bytes budget must be non-zero"));
    }
    if subtotal > budget.total_bytes {
        return Err(schema_error(format!(
            "context class budgets total {subtotal}, exceeding total_bytes {}",
            budget.total_bytes
        )));
    }
    Ok(())
}

fn truncate_with_label(input: &str, budget: usize) -> (String, bool) {
    if input.len() <= budget {
        return (input.to_string(), false);
    }
    let marker = format!(
        "\n[context truncated: original_bytes={} included_budget={} sha256={}]\n",
        input.len(),
        budget,
        sha256_hex(input.as_bytes())
    );
    if marker.len() >= budget {
        return (String::new(), true);
    }
    let available = budget - marker.len();
    let head_budget = available / 2;
    let tail_budget = available - head_budget;
    let head_end = floor_char_boundary(input, head_budget);
    let tail_start = ceil_char_boundary(input, input.len().saturating_sub(tail_budget));
    (
        format!("{}{}{}", &input[..head_end], marker, &input[tail_start..]),
        true,
    )
}

fn render_context_section(
    label: &str,
    original_bytes: usize,
    included_bytes: usize,
    truncated: bool,
    body: &str,
) -> String {
    format!(
        "----- BEGIN CONTEXT label={label} original_bytes={original_bytes} included_bytes={included_bytes} truncated={truncated} -----\n{body}\n----- END CONTEXT label={label} -----"
    )
}

fn redact_secret_like_values(input: &str) -> String {
    input
        .lines()
        .map(redact_secret_like_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_secret_like_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let key_like = ["api_key", "apikey", "token", "secret", "password"]
        .iter()
        .any(|needle| lower.contains(needle));
    if key_like && (line.contains('=') || line.contains(':')) {
        if let Some(index) = line.find('=') {
            return format!("{}=[REDACTED]", &line[..index]);
        }
        if let Some(index) = line.find(':') {
            return format!("{}: [REDACTED]", &line[..index]);
        }
    }

    line.split_whitespace()
        .map(|word| {
            if looks_like_secret_token(word) {
                "[REDACTED]".to_string()
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_secret_token(word: &str) -> bool {
    let trimmed =
        word.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-');
    trimmed.starts_with("sk-")
        || trimmed.starts_with("ghp_")
        || trimmed.starts_with("github_pat_")
        || trimmed.starts_with("xoxb-")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn floor_char_boundary(input: &str, mut index: usize) -> usize {
    index = index.min(input.len());
    while !input.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(input: &str, mut index: usize) -> usize {
    index = index.min(input.len());
    while !input.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn schema_error(message: impl Into<String>) -> HarnessError {
    HarnessError::Usage(format!("invalid schema: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_planner_json() -> String {
        serde_json::json!({
            "schema_version": 1,
            "objective": {
                "title": "Create Rust clone of Volt CLI",
                "summary": "Implement a Rust CLI with the core behavior.",
                "acceptance_criteria": ["Rust CLI builds successfully"],
                "validation_commands": ["cargo test"]
            },
            "tasks": [
                {
                    "task_key": "inspect_volt_cli",
                    "title": "Inspect Volt CLI command surface",
                    "goal": "Determine command groups and options.",
                    "validation": ["cargo test command_surface_known"],
                    "depends_on": [],
                    "owned_paths": ["src/cli"],
                    "parallel_group": "discovery"
                },
                {
                    "task_key": "implement_cli",
                    "title": "Implement CLI",
                    "goal": "Build the command surface.",
                    "validation": ["cargo test cli_surface"],
                    "depends_on": ["inspect_volt_cli"],
                    "owned_paths": ["src/main.rs"],
                    "parallel_group": "implementation"
                }
            ],
            "risks": [],
            "final_verification": ["Run all objective validation commands"]
        })
        .to_string()
    }

    fn planner_value() -> serde_json::Value {
        serde_json::from_str(&valid_planner_json()).unwrap()
    }

    #[test]
    fn valid_planner_schema_parses() {
        let parsed = parse_planner_response(&valid_planner_json()).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.tasks[1].depends_on, vec!["inspect_volt_cli"]);
    }

    #[test]
    fn planner_output_rejects_unknown_fields() {
        let mut value = planner_value();
        value["unexpected"] = serde_json::json!(true);
        assert!(parse_planner_response(&value.to_string()).is_err());
    }

    #[test]
    fn planner_output_rejects_markdown_wrappers() {
        let wrapped = format!("```json\n{}\n```", valid_planner_json());
        assert!(parse_planner_response(&wrapped).is_err());
    }

    #[test]
    fn planner_output_rejects_duplicate_task_keys() {
        let mut value = planner_value();
        value["tasks"][1]["task_key"] = serde_json::json!("inspect_volt_cli");
        assert!(parse_planner_response(&value.to_string()).is_err());
    }

    #[test]
    fn planner_output_rejects_cycles() {
        let mut value = planner_value();
        value["tasks"][0]["depends_on"] = serde_json::json!(["implement_cli"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
    }

    #[test]
    fn planner_output_rejects_invalid_dependencies() {
        let mut value = planner_value();
        value["tasks"][1]["depends_on"] = serde_json::json!(["missing_task"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
    }

    #[test]
    fn planner_output_rejects_path_escapes() {
        let mut value = planner_value();
        value["tasks"][0]["owned_paths"] = serde_json::json!(["../outside"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
        value["tasks"][0]["owned_paths"] = serde_json::json!(["..\\outside"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
        value["tasks"][0]["owned_paths"] = serde_json::json!([".git/config"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
        value["tasks"][0]["owned_paths"] = serde_json::json!([".git\\config"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
        value["tasks"][0]["owned_paths"] = serde_json::json!([".harness/state.db"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
        value["tasks"][0]["owned_paths"] = serde_json::json!([".harness\\state.db"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
        value["tasks"][0]["owned_paths"] = serde_json::json!(["src/lib.rs\u{0}"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
        value["tasks"][0]["owned_paths"] = serde_json::json!(["C:\\repo\\src"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
        value["tasks"][0]["owned_paths"] = serde_json::json!(["\\\\server\\share"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
        value["tasks"][0]["owned_paths"] = serde_json::json!(["~/repo/src"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
        value["tasks"][0]["owned_paths"] = serde_json::json!(["/tmp/outside"]);
        assert!(parse_planner_response(&value.to_string()).is_err());
    }

    #[test]
    fn planner_output_can_be_validated_against_repo_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "").unwrap();

        assert!(parse_planner_response_for_repo(&valid_planner_json(), temp.path()).is_ok());
    }

    #[test]
    fn planner_output_accepts_dot_as_whole_repo_owned_path() {
        let temp = tempfile::tempdir().unwrap();
        let mut value = planner_value();
        value["tasks"][0]["owned_paths"] = serde_json::json!(["."]);

        let parsed = parse_planner_response_for_repo(&value.to_string(), temp.path()).unwrap();

        assert_eq!(parsed.tasks[0].owned_paths, vec!["."]);
    }

    #[cfg(unix)]
    #[test]
    fn planner_repo_validation_rejects_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), temp.path().join("linked-out")).unwrap();

        let mut value = planner_value();
        value["tasks"][0]["owned_paths"] = serde_json::json!(["linked-out/file.txt"]);
        assert!(parse_planner_response(&value.to_string()).is_ok());
        assert!(parse_planner_response_for_repo(&value.to_string(), temp.path()).is_err());
    }

    #[test]
    fn planner_output_enforces_task_count_limit() {
        let mut value = planner_value();
        let task = value["tasks"][0].clone();
        value["tasks"] = serde_json::Value::Array(vec![task; MAX_PLANNER_TASKS + 1]);
        assert!(parse_planner_response(&value.to_string()).is_err());
    }

    fn valid_resolver_json() -> String {
        serde_json::json!({
            "schema_version": 1,
            "diagnosis": "The local worker missed nested subcommands.",
            "recommended_steps": [
                "Inspect command registration and add the missing nested subcommand before retrying."
            ],
            "constraints": ["Do not change authentication behavior"],
            "validation_focus": ["cargo test cli_surface"]
        })
        .to_string()
    }

    #[test]
    fn valid_resolver_schema_parses() {
        let parsed = parse_ticket_resolver_response(&valid_resolver_json()).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.validation_focus, vec!["cargo test cli_surface"]);
    }

    #[test]
    fn resolver_output_rejects_patch_like_content() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_resolver_json()).unwrap();
        value["recommended_steps"][0] = serde_json::json!("diff --git a/src/lib.rs b/src/lib.rs");
        assert!(parse_ticket_resolver_response(&value.to_string()).is_err());
    }

    #[test]
    fn resolver_output_rejects_diff_like_content() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_resolver_json()).unwrap();
        value["diagnosis"] = serde_json::json!("--- a\n+++ b\n@@ -1 +1 @@");
        assert!(parse_ticket_resolver_response(&value.to_string()).is_err());
    }

    #[test]
    fn resolver_output_rejects_script_like_content() {
        let mut value: serde_json::Value = serde_json::from_str(&valid_resolver_json()).unwrap();
        value["recommended_steps"][0] = serde_json::json!("```bash\ncargo test\n```");
        assert!(parse_ticket_resolver_response(&value.to_string()).is_err());
        value["recommended_steps"][0] = serde_json::json!("$ cargo test");
        assert!(parse_ticket_resolver_response(&value.to_string()).is_err());
        value["recommended_steps"][0] =
            serde_json::json!("Run:\n1. cargo test\n2. cargo fmt --check");
        assert!(parse_ticket_resolver_response(&value.to_string()).is_err());
        value["recommended_steps"][0] =
            serde_json::json!("Try these commands:\n- git status\n- ./scripts/check");
        assert!(parse_ticket_resolver_response(&value.to_string()).is_err());
        value["recommended_steps"][0] =
            serde_json::json!("Do these:\n1. rm -rf target\n2. chmod +x validate.sh");
        assert!(parse_ticket_resolver_response(&value.to_string()).is_err());
        value["recommended_steps"][0] =
            serde_json::json!("Do these:\n- cd src\n- sed -i s/foo/bar/g main.rs");
        assert!(parse_ticket_resolver_response(&value.to_string()).is_err());
    }

    #[test]
    fn context_pack_is_deterministic_for_same_inputs() {
        let request = context_pack_request();
        let first = pack_context(request.clone()).unwrap();
        let second = pack_context(request).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.manifest.schema_version, 1);
    }

    #[test]
    fn context_pack_records_included_and_omitted_sections() {
        let mut request = context_pack_request();
        request.budget.conversation_bytes = 1;
        request.budget.total_bytes = request.budget.objective_bytes
            + request.budget.conversation_bytes
            + request.budget.state_bytes
            + request.budget.artifact_excerpt_bytes
            + request.budget.schema_bytes;
        request.sections.push(ContextSection {
            label: "older_events".to_string(),
            priority: 99,
            budget_class: ContextBudgetClass::Conversation,
            required: false,
            body: "older".to_string(),
        });

        let packed = pack_context(request).unwrap();
        assert!(
            packed
                .manifest
                .included_sections
                .iter()
                .any(|section| section.label == "objective")
        );
        assert!(
            packed
                .manifest
                .omitted_sections
                .iter()
                .any(|section| section.label == "older_events")
        );
        assert!(packed.body.contains("original_bytes="));
        assert!(packed.body.contains("included_bytes="));
    }

    #[test]
    fn context_pack_redacts_secret_looking_data() {
        let mut request = context_pack_request();
        request.sections.push(ContextSection {
            label: "secrets".to_string(),
            priority: 3,
            budget_class: ContextBudgetClass::State,
            required: false,
            body: "OPENAI_API_KEY=sk-testsecret".to_string(),
        });
        let packed = pack_context(request).unwrap();
        assert!(!packed.body.contains("sk-testsecret"));
        assert!(packed.body.contains("[REDACTED]"));
    }

    fn context_pack_request() -> ContextPackRequest {
        ContextPackRequest {
            budget: ContextBudget {
                total_bytes: 512,
                objective_bytes: 128,
                conversation_bytes: 64,
                state_bytes: 128,
                artifact_excerpt_bytes: 128,
                schema_bytes: 64,
            },
            sections: vec![
                ContextSection {
                    label: "state".to_string(),
                    priority: 4,
                    budget_class: ContextBudgetClass::State,
                    required: false,
                    body: "task status ready".to_string(),
                },
                ContextSection {
                    label: "objective".to_string(),
                    priority: 2,
                    budget_class: ContextBudgetClass::Objective,
                    required: true,
                    body: "build the CLI".to_string(),
                },
            ],
            artifacts: vec![ContextArtifact {
                artifact_id: "obj_art_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
                label: "validation stderr".to_string(),
                priority: 8,
                body: "error: missing subcommand".repeat(20),
            }],
        }
    }
}
