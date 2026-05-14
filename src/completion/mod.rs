pub mod shell;

use crate::domain::{RunId, TaskId, TaskStatus, TicketId, TicketStatus};
use crate::error::HarnessResult;
use crate::runtime::{
    CommandCatalog, CommandNodeSpec, MetaCommandSpec, OptionSpec, PositionalSpec, StateQueryKind,
    ValueKind, ValueSource, ValueSpec,
};
use crate::security::{DefaultRedactor, Redactor};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteState {
    None,
    Single,
    Double,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionToken {
    pub raw: String,
    pub value: String,
    pub start: usize,
    pub end: usize,
    pub quote: Option<char>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContextKind {
    CommandOrSubcommand,
    OptionName,
    OptionValue {
        option: String,
        value_kind: ValueKind,
    },
    PositionalValue {
        positional: String,
        value_kind: ValueKind,
    },
    MetaCommand,
    ShellEscape,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionParse {
    pub tokens: Vec<CompletionToken>,
    pub cursor: usize,
    pub cursor_token: Option<usize>,
    pub trailing_empty_token: bool,
    pub active_command_path: Vec<String>,
    pub active_fragment: String,
    pub context: CompletionContextKind,
    pub quote_state: QuoteState,
}

#[derive(Clone)]
pub struct CompletionContext<'a> {
    pub state: &'a dyn CompletionStateView,
    pub repo: Option<PathBuf>,
    pub catalog: &'a CommandCatalog,
}

pub trait CompletionStateView {
    fn tasks_for_completion(
        &self,
        scope: TaskCompletionScope,
    ) -> HarnessResult<Vec<TaskCompletionItem>>;

    fn tickets_for_completion(
        &self,
        scope: TicketCompletionScope,
    ) -> HarnessResult<Vec<TicketCompletionItem>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCompletionScope {
    pub statuses: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketCompletionScope {
    pub statuses: Vec<&'static str>,
    pub task_id: Option<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCompletionItem {
    pub id: TaskId,
    pub status: TaskStatus,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketCompletionItem {
    pub id: TicketId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub status: TicketStatus,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Command,
    Option,
    Value,
    TaskId,
    TicketId,
    MetaCommand,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCandidate {
    pub replacement: String,
    pub display: String,
    pub detail: String,
    pub kind: CompletionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionStatus {
    Ready,
    Loading,
    Error(String),
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionSet {
    pub replacement_start: usize,
    pub replacement_end: usize,
    pub candidates: Vec<CompletionCandidate>,
    pub longest_common_prefix: Option<String>,
    pub status: CompletionStatus,
    pub hint: Option<CompletionCandidate>,
    pub readiness: CommandReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandReadiness {
    Ready,
    Incomplete { missing: Vec<String>, hint: String },
    Invalid { diagnostic: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCacheKey {
    pub repo_identity: String,
    pub command_path: Vec<String>,
    pub value_kind: ValueKind,
    pub task_scope: Option<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCacheEntry {
    pub candidates: Vec<CompletionCandidate>,
    pub status: CompletionStatus,
}

#[derive(Debug)]
pub struct CompletionCache {
    capacity: usize,
    entries: Mutex<VecDeque<(CompletionCacheKey, CompletionCacheEntry)>>,
}

impl CompletionCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Mutex::new(VecDeque::new()),
        }
    }

    pub fn get(&self, key: &CompletionCacheKey) -> Option<CompletionCacheEntry> {
        let mut entries = self.entries.lock().expect("completion cache poisoned");
        let index = entries.iter().position(|(entry_key, _)| entry_key == key)?;
        let (entry_key, entry) = entries.remove(index)?;
        entries.push_back((entry_key, entry.clone()));
        Some(entry)
    }

    pub fn insert(&self, key: CompletionCacheKey, entry: CompletionCacheEntry) {
        if self.capacity == 0 {
            return;
        }
        let mut entries = self.entries.lock().expect("completion cache poisoned");
        if let Some(index) = entries.iter().position(|(entry_key, _)| entry_key == &key) {
            entries.remove(index);
        }
        entries.push_back((key, entry));
        while entries.len() > self.capacity {
            entries.pop_front();
        }
    }

    pub fn clear(&self) {
        self.entries
            .lock()
            .expect("completion cache poisoned")
            .clear();
    }

    pub fn invalidate_repo(&self, repo_identity: &str) {
        self.entries
            .lock()
            .expect("completion cache poisoned")
            .retain(|(key, _)| key.repo_identity != repo_identity);
    }
}

impl Default for CompletionCache {
    fn default() -> Self {
        Self::new(64)
    }
}

pub trait CompleterEngine {
    fn complete(
        &self,
        line: &str,
        cursor: usize,
        context: &CompletionContext<'_>,
    ) -> HarnessResult<CompletionSet>;
}

#[derive(Debug, Default)]
pub struct CompletionEngine {
    cache: CompletionCache,
}

impl CompletionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_cache_capacity(capacity: usize) -> Self {
        Self {
            cache: CompletionCache::new(capacity),
        }
    }

    pub fn cache(&self) -> &CompletionCache {
        &self.cache
    }

    pub fn parse(&self, line: &str, cursor: usize, catalog: &CommandCatalog) -> CompletionParse {
        CompletionAnalyzer::new(line, cursor, catalog).parse()
    }
}

impl CompleterEngine for CompletionEngine {
    fn complete(
        &self,
        line: &str,
        cursor: usize,
        context: &CompletionContext<'_>,
    ) -> HarnessResult<CompletionSet> {
        let parse = self.parse(line, cursor, context.catalog);
        let analysis = CompletionAnalyzer::new(line, cursor, context.catalog).analyze();
        let readiness = readiness_from_analysis(&analysis);
        let (replacement_start, replacement_end) = replacement_span(&analysis);
        let mut status = CompletionStatus::Ready;
        let mut candidates = Vec::new();
        let mut hint = None;

        match &parse.context {
            CompletionContextKind::ShellEscape => {
                hint = Some(hint_candidate(
                    "shell escape",
                    "Run from the repository root without harness completions.",
                ));
            }
            CompletionContextKind::CommandOrSubcommand | CompletionContextKind::MetaCommand => {
                candidates.extend(command_candidates(&analysis, context.catalog));
            }
            CompletionContextKind::OptionName => {
                candidates.extend(option_candidates(&analysis));
            }
            CompletionContextKind::OptionValue { option, .. } => {
                if let Some(spec) = lookup_option(analysis.node, context.catalog, option) {
                    let dynamic_status = self.value_candidates(
                        spec.value.as_ref(),
                        &analysis,
                        context,
                        &mut candidates,
                    );
                    if let Some(dynamic_status) = dynamic_status {
                        status = dynamic_status;
                    }
                    if candidates.is_empty() {
                        hint = Some(value_hint(spec.value.as_ref(), spec.value_name));
                    }
                }
            }
            CompletionContextKind::PositionalValue { positional, .. } => {
                if let Some(spec) = positional_by_name(analysis.node, positional) {
                    let dynamic_status = self.value_candidates(
                        Some(&spec.value),
                        &analysis,
                        context,
                        &mut candidates,
                    );
                    if let Some(dynamic_status) = dynamic_status {
                        status = dynamic_status;
                    }
                    if candidates.is_empty() {
                        hint = Some(value_hint(Some(&spec.value), Some(spec.name)));
                    }
                }
            }
            CompletionContextKind::Unknown => {
                hint = Some(hint_candidate(
                    "unknown command",
                    "No completion is available.",
                ));
            }
        }

        let fragment = parse.active_fragment.as_str();
        candidates.retain(|candidate| candidate.replacement.starts_with(fragment));
        rank_candidates(&mut candidates);
        redact_candidates(&mut candidates);
        if let Some(hint) = &mut hint {
            redact_candidate(hint);
        }

        let longest_common_prefix = longest_common_prefix(&candidates);
        Ok(CompletionSet {
            replacement_start,
            replacement_end,
            candidates,
            longest_common_prefix,
            status,
            hint,
            readiness,
        })
    }
}

impl CompletionEngine {
    fn value_candidates(
        &self,
        value: Option<&ValueSpec>,
        analysis: &Analysis<'_>,
        context: &CompletionContext<'_>,
        candidates: &mut Vec<CompletionCandidate>,
    ) -> Option<CompletionStatus> {
        let Some(value) = value else {
            return None;
        };

        match &value.source {
            ValueSource::Static(values) => {
                candidates.extend(values.iter().map(|value| CompletionCandidate {
                    replacement: (*value).to_string(),
                    display: (*value).to_string(),
                    detail: String::new(),
                    kind: CompletionKind::Value,
                }));
                None
            }
            ValueSource::StateQuery(StateQueryKind::TaskId { statuses }) => {
                let key = CompletionCacheKey {
                    repo_identity: repo_identity(context.repo.as_ref()),
                    command_path: analysis.path.clone(),
                    value_kind: value.kind,
                    task_scope: None,
                };
                if let Some(entry) = self.cache.get(&key) {
                    candidates.extend(entry.candidates);
                    return Some(CompletionStatus::Stale);
                }
                let scope = TaskCompletionScope {
                    statuses: statuses.to_vec(),
                };
                match context.state.tasks_for_completion(scope) {
                    Ok(items) => {
                        let mut dynamic = items.into_iter().map(task_candidate).collect::<Vec<_>>();
                        redact_candidates(&mut dynamic);
                        self.cache.insert(
                            key,
                            CompletionCacheEntry {
                                candidates: dynamic.clone(),
                                status: CompletionStatus::Ready,
                            },
                        );
                        candidates.extend(dynamic);
                        None
                    }
                    Err(err) => Some(CompletionStatus::Error(err.to_string())),
                }
            }
            ValueSource::StateQuery(StateQueryKind::TicketId {
                statuses,
                scoped_to_task_arg,
            }) => {
                let task_id = if *scoped_to_task_arg {
                    scoped_task_id(analysis)
                } else {
                    None
                };
                let key = CompletionCacheKey {
                    repo_identity: repo_identity(context.repo.as_ref()),
                    command_path: analysis.path.clone(),
                    value_kind: value.kind,
                    task_scope: task_id.clone(),
                };
                if let Some(entry) = self.cache.get(&key) {
                    candidates.extend(entry.candidates);
                    return Some(CompletionStatus::Stale);
                }
                let scope = TicketCompletionScope {
                    statuses: statuses.to_vec(),
                    task_id,
                };
                match context.state.tickets_for_completion(scope) {
                    Ok(items) => {
                        let mut dynamic =
                            items.into_iter().map(ticket_candidate).collect::<Vec<_>>();
                        redact_candidates(&mut dynamic);
                        self.cache.insert(
                            key,
                            CompletionCacheEntry {
                                candidates: dynamic.clone(),
                                status: CompletionStatus::Ready,
                            },
                        );
                        candidates.extend(dynamic);
                        None
                    }
                    Err(err) => Some(CompletionStatus::Error(err.to_string())),
                }
            }
            ValueSource::HintOnly | ValueSource::NoCompletion | ValueSource::FilesystemPath => None,
        }
    }
}

#[derive(Debug, Clone)]
struct Analysis<'a> {
    parse: CompletionParse,
    catalog: &'a CommandCatalog,
    node: Option<&'a CommandNodeSpec>,
    path: Vec<String>,
    operands: Vec<(String, String)>,
    seen_options: Vec<String>,
    duplicate_options: Vec<String>,
    pending_option: Option<&'a OptionSpec>,
    unknown_option: Option<String>,
    unknown_command: Option<String>,
}

struct CompletionAnalyzer<'a> {
    line: &'a str,
    cursor: usize,
    catalog: &'a CommandCatalog,
}

impl<'a> CompletionAnalyzer<'a> {
    fn new(line: &'a str, cursor: usize, catalog: &'a CommandCatalog) -> Self {
        Self {
            line,
            cursor: clamp_cursor(line, cursor),
            catalog,
        }
    }

    fn parse(&self) -> CompletionParse {
        self.analyze().parse
    }

    fn analyze(&self) -> Analysis<'a> {
        let (tokens, quote_state) = tokenize_tolerant(self.line);
        let cursor_token = tokens
            .iter()
            .position(|token| token.start <= self.cursor && self.cursor <= token.end);
        let trailing_empty_token = self.cursor > 0
            && self
                .line
                .as_bytes()
                .get(self.cursor.saturating_sub(1))
                .is_some_and(u8::is_ascii_whitespace)
            && quote_state == QuoteState::None;
        let mut active_fragment = cursor_token
            .and_then(|index| tokens.get(index))
            .map(|token| token_value_prefix(self.line, token, self.cursor))
            .unwrap_or_default();
        if let Some(index) = cursor_token {
            if let Some(token) = tokens.get(index) {
                if let Some((_, value_start)) = split_long_option_assignment(token) {
                    if self.cursor >= value_start {
                        active_fragment = self.line[value_start..self.cursor].to_string();
                    }
                }
            }
        }
        let scan_end = if trailing_empty_token {
            tokens
                .iter()
                .take_while(|token| token.end <= self.cursor)
                .count()
        } else {
            cursor_token.unwrap_or_else(|| {
                tokens
                    .iter()
                    .take_while(|token| token.end <= self.cursor)
                    .count()
            })
        };
        let scan = scan_tokens(self.catalog, &tokens[..scan_end]);
        let context = self.context_for(
            &tokens,
            cursor_token,
            trailing_empty_token,
            &active_fragment,
            &scan,
        );
        let path = scan.path.clone();

        Analysis {
            parse: CompletionParse {
                tokens,
                cursor: self.cursor,
                cursor_token,
                trailing_empty_token,
                active_command_path: path.clone(),
                active_fragment,
                context,
                quote_state,
            },
            catalog: self.catalog,
            node: scan.node,
            path,
            operands: scan.operands,
            seen_options: scan.seen_options,
            duplicate_options: scan.duplicate_options,
            pending_option: scan.pending_option,
            unknown_option: scan.unknown_option,
            unknown_command: scan.unknown_command,
        }
    }

    fn context_for(
        &self,
        tokens: &[CompletionToken],
        cursor_token: Option<usize>,
        trailing_empty_token: bool,
        active_fragment: &str,
        scan: &ScanResult<'a>,
    ) -> CompletionContextKind {
        if self.line.trim_start().starts_with('!') {
            return CompletionContextKind::ShellEscape;
        }

        if scan.unknown_command.is_some() || scan.unknown_option.is_some() {
            return CompletionContextKind::Unknown;
        }

        if let Some(option) = scan.pending_option {
            return CompletionContextKind::OptionValue {
                option: option.long.to_string(),
                value_kind: option
                    .value
                    .as_ref()
                    .map(|value| value.kind)
                    .unwrap_or(ValueKind::FreeText),
            };
        }

        if !trailing_empty_token {
            if let Some(index) = cursor_token {
                let token = &tokens[index];
                if let Some((name, value_start)) = split_long_option_assignment(token) {
                    if self.cursor >= value_start {
                        if let Some(option) = lookup_option(scan.node, self.catalog, name) {
                            return CompletionContextKind::OptionValue {
                                option: option.long.to_string(),
                                value_kind: option
                                    .value
                                    .as_ref()
                                    .map(|value| value.kind)
                                    .unwrap_or(ValueKind::FreeText),
                            };
                        }
                    }
                }
                if active_fragment.starts_with("--") && !scan.options_ended {
                    return CompletionContextKind::OptionName;
                }
            }
        }

        if scan.node.is_none() {
            if root_meta_matches(active_fragment, self.catalog) {
                return CompletionContextKind::MetaCommand;
            }
            return CompletionContextKind::CommandOrSubcommand;
        }

        if scan.node.is_some_and(|node| !node.children.is_empty()) {
            return CompletionContextKind::CommandOrSubcommand;
        }

        if !scan.options_ended && trailing_empty_token {
            if next_positional(scan.node, &scan.operands, &scan.seen_options).is_none() {
                return CompletionContextKind::OptionName;
            }
        }

        if let Some(positional) = next_positional(scan.node, &scan.operands, &scan.seen_options) {
            return CompletionContextKind::PositionalValue {
                positional: positional.name.to_string(),
                value_kind: positional.value.kind,
            };
        }

        if !scan.options_ended {
            CompletionContextKind::OptionName
        } else {
            CompletionContextKind::Unknown
        }
    }
}

#[derive(Debug)]
struct ScanResult<'a> {
    node: Option<&'a CommandNodeSpec>,
    path: Vec<String>,
    operands: Vec<(String, String)>,
    seen_options: Vec<String>,
    duplicate_options: Vec<String>,
    pending_option: Option<&'a OptionSpec>,
    unknown_option: Option<String>,
    unknown_command: Option<String>,
    options_ended: bool,
}

fn scan_tokens<'a>(catalog: &'a CommandCatalog, tokens: &[CompletionToken]) -> ScanResult<'a> {
    let mut node = None;
    let mut path = Vec::new();
    let mut operands = Vec::new();
    let mut seen_options = Vec::new();
    let mut duplicate_options = Vec::new();
    let mut pending_option = None;
    let mut unknown_option = None;
    let mut unknown_command = None;
    let mut options_ended = false;
    let mut index = 0;

    if tokens
        .first()
        .is_some_and(|token| token.value == catalog.tree().name)
    {
        index = 1;
    }

    while index < tokens.len() {
        let token = &tokens[index];
        if !options_ended && token.value == "--" {
            options_ended = true;
            index += 1;
            continue;
        }

        if !options_ended && token.value.starts_with("--") {
            let (name, assigned_value) = option_name_and_value(&token.value);
            let Some(option) = lookup_option(node, catalog, name) else {
                unknown_option = Some(token.value.clone());
                break;
            };
            push_seen_option(option, &mut seen_options, &mut duplicate_options);
            if option.value.is_some() && assigned_value.is_none() {
                if index + 1 >= tokens.len() {
                    pending_option = Some(option);
                    break;
                }
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }

        if node.is_none() {
            if let Some(child) = catalog
                .tree()
                .commands
                .iter()
                .find(|candidate| command_name_matches(candidate, &token.value))
            {
                node = Some(child);
                path.push(child.name.to_string());
                index += 1;
                continue;
            }
            if catalog
                .tree()
                .meta_commands
                .iter()
                .any(|meta| meta_name_matches(meta, &token.value))
            {
                path.push(token.value.clone());
                index += 1;
                continue;
            }
            unknown_command = Some(token.value.clone());
            break;
        }

        if let Some(current) = node {
            if !current.children.is_empty() {
                if let Some(child) = current
                    .children
                    .iter()
                    .find(|candidate| command_name_matches(candidate, &token.value))
                {
                    node = Some(child);
                    path.push(child.name.to_string());
                    index += 1;
                    continue;
                }
                unknown_command = Some(token.value.clone());
                break;
            }
        }

        if let Some(positional) = next_positional(node, &operands, &seen_options) {
            operands.push((positional.name.to_string(), token.value.clone()));
            index += 1;
        } else {
            unknown_command = Some(token.value.clone());
            break;
        }
    }

    ScanResult {
        node,
        path,
        operands,
        seen_options,
        duplicate_options,
        pending_option,
        unknown_option,
        unknown_command,
        options_ended,
    }
}

fn tokenize_tolerant(input: &str) -> (Vec<CompletionToken>, QuoteState) {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut raw_start = None;
    let mut raw_end = 0;
    let mut quote = QuoteState::None;
    let mut token_quote = None;
    let mut chars = input.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        match quote {
            QuoteState::Single => {
                raw_end = index + ch.len_utf8();
                if ch == '\'' {
                    quote = QuoteState::None;
                } else {
                    current.push(ch);
                }
            }
            QuoteState::Double => {
                raw_end = index + ch.len_utf8();
                match ch {
                    '"' => quote = QuoteState::None,
                    '\\' => {
                        if let Some((next_index, next)) = chars.next() {
                            raw_end = next_index + next.len_utf8();
                            current.push(next);
                        } else {
                            current.push('\\');
                        }
                    }
                    _ => current.push(ch),
                }
            }
            QuoteState::None => match ch {
                '\'' | '"' => {
                    if raw_start.is_none() {
                        raw_start = Some(index);
                        token_quote = Some(ch);
                    }
                    raw_end = index + ch.len_utf8();
                    quote = if ch == '\'' {
                        QuoteState::Single
                    } else {
                        QuoteState::Double
                    };
                }
                '\\' => {
                    if raw_start.is_none() {
                        raw_start = Some(index);
                    }
                    if let Some((next_index, next)) = chars.next() {
                        raw_end = next_index + next.len_utf8();
                        current.push(next);
                    } else {
                        raw_end = index + ch.len_utf8();
                        current.push('\\');
                    }
                }
                ch if ch.is_whitespace() => {
                    if let Some(start) = raw_start.take() {
                        tokens.push(CompletionToken {
                            raw: input[start..raw_end].to_string(),
                            value: std::mem::take(&mut current),
                            start,
                            end: raw_end,
                            quote: token_quote.take(),
                        });
                    }
                }
                _ => {
                    if raw_start.is_none() {
                        raw_start = Some(index);
                    }
                    raw_end = index + ch.len_utf8();
                    current.push(ch);
                }
            },
        }
    }

    if let Some(start) = raw_start {
        tokens.push(CompletionToken {
            raw: input[start..raw_end].to_string(),
            value: current,
            start,
            end: raw_end,
            quote: token_quote,
        });
    }

    (tokens, quote)
}

fn token_value_prefix(line: &str, token: &CompletionToken, cursor: usize) -> String {
    let end = cursor.clamp(token.start, token.end);
    let fragment = &line[token.start..end];
    let (tokens, _) = tokenize_tolerant(fragment);
    tokens
        .last()
        .map(|token| token.value.clone())
        .unwrap_or_default()
}

fn command_candidates(
    analysis: &Analysis<'_>,
    catalog: &CommandCatalog,
) -> Vec<CompletionCandidate> {
    if analysis.node.is_none() {
        let mut candidates = catalog
            .tree()
            .commands
            .iter()
            .filter(|node| !node.hidden)
            .map(|node| CompletionCandidate {
                replacement: node.name.to_string(),
                display: node.name.to_string(),
                detail: node.about.to_string(),
                kind: CompletionKind::Command,
            })
            .collect::<Vec<_>>();
        candidates.extend(
            catalog
                .tree()
                .meta_commands
                .iter()
                .map(|meta| CompletionCandidate {
                    replacement: meta.name.to_string(),
                    display: meta.name.to_string(),
                    detail: meta.about.to_string(),
                    kind: CompletionKind::MetaCommand,
                }),
        );
        return candidates;
    }

    analysis
        .node
        .into_iter()
        .flat_map(|node| node.children)
        .filter(|node| !node.hidden)
        .map(|node| CompletionCandidate {
            replacement: node.name.to_string(),
            display: node.name.to_string(),
            detail: node.about.to_string(),
            kind: CompletionKind::Command,
        })
        .collect()
}

fn option_candidates(analysis: &Analysis<'_>) -> Vec<CompletionCandidate> {
    let mut options = Vec::new();
    if let Some(node) = analysis.node {
        options.extend(node.options.iter());
    }
    options.extend(analysis.catalog.tree().globals.iter());

    let seen = analysis
        .seen_options
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    options
        .into_iter()
        .filter(|option| option.repeatable || !seen.contains(option.long))
        .map(|option| CompletionCandidate {
            replacement: format!("--{}", option.long),
            display: format!("--{}", option.long),
            detail: option_detail(option),
            kind: CompletionKind::Option,
        })
        .collect()
}

fn lookup_option<'a>(
    node: Option<&'a CommandNodeSpec>,
    catalog: &'a CommandCatalog,
    name: &str,
) -> Option<&'a OptionSpec> {
    let name = name.strip_prefix("--").unwrap_or(name);
    node.into_iter()
        .flat_map(|node| node.options)
        .chain(catalog.tree().globals.iter())
        .find(|option| option.long == name)
}

fn positional_by_name<'a>(
    node: Option<&'a CommandNodeSpec>,
    name: &str,
) -> Option<&'a PositionalSpec> {
    node.into_iter()
        .flat_map(|node| node.positionals)
        .find(|positional| positional.name == name)
}

fn next_positional<'a>(
    node: Option<&'a CommandNodeSpec>,
    operands: &[(String, String)],
    seen_options: &[String],
) -> Option<&'a PositionalSpec> {
    let node = node?;
    node.positionals.iter().find(|positional| {
        if operands.iter().any(|(name, _)| name == positional.name) && !positional.repeatable {
            return false;
        }
        if positional
            .conflicts_with
            .iter()
            .any(|name| seen_options.iter().any(|seen| seen == name))
        {
            return false;
        }
        true
    })
}

fn readiness_from_analysis(analysis: &Analysis<'_>) -> CommandReadiness {
    let scan = scan_for_readiness(analysis);

    if analysis.parse.quote_state != QuoteState::None {
        return CommandReadiness::Incomplete {
            missing: vec!["closing quote".to_string()],
            hint: "Close the current quote before running the command.".to_string(),
        };
    }
    if let Some(option) = scan.pending_option {
        return CommandReadiness::Incomplete {
            missing: vec![format!("value for --{}", option.long)],
            hint: format!("Provide a value for --{}.", option.long),
        };
    }
    if let Some(command) = &scan.unknown_command {
        return CommandReadiness::Invalid {
            diagnostic: format!("unknown command or argument {command:?}"),
        };
    }
    if let Some(option) = &scan.unknown_option {
        return CommandReadiness::Invalid {
            diagnostic: format!("unknown option {option:?}"),
        };
    }
    if let Some(duplicate) = scan.duplicate_options.first() {
        return CommandReadiness::Invalid {
            diagnostic: format!("--{duplicate} cannot be repeated"),
        };
    }
    if exact_meta_command(&scan.path, analysis.catalog) {
        return CommandReadiness::Ready;
    }
    let Some(node) = scan.node else {
        return CommandReadiness::Incomplete {
            missing: vec!["command".to_string()],
            hint: "Choose a command.".to_string(),
        };
    };
    if !node.children.is_empty() {
        return CommandReadiness::Incomplete {
            missing: vec![format!("{} subcommand", node.name)],
            hint: format!("Choose a {} subcommand.", node.name),
        };
    }

    let present = present_names(&scan);
    if let Some(active_positional) = active_positional_for_readiness(analysis) {
        for conflict in active_positional.conflicts_with {
            if present.contains(*conflict) {
                return CommandReadiness::Invalid {
                    diagnostic: format!("{} conflicts with {conflict}", active_positional.name),
                };
            }
        }
    }
    for option in node.options {
        if present.contains(option.long) {
            for conflict in option.conflicts_with {
                if present.contains(*conflict) {
                    return CommandReadiness::Invalid {
                        diagnostic: format!("--{} conflicts with {conflict}", option.long),
                    };
                }
            }
            for required in option.requires {
                if !present.contains(*required) {
                    return CommandReadiness::Incomplete {
                        missing: vec![format!("--{required}")],
                        hint: format!("--{} requires --{required}.", option.long),
                    };
                }
            }
        }
    }

    let mut missing = Vec::new();
    for positional in node.positionals {
        let has_value = scan
            .operands
            .iter()
            .any(|(name, value)| name == positional.name && !value.is_empty());
        let unless_present = positional
            .required_unless_present
            .iter()
            .any(|name| present.contains(*name));
        if positional.required && !has_value {
            missing.push(positional.name.to_string());
        } else if !positional.required
            && !has_value
            && !positional.required_unless_present.is_empty()
            && !unless_present
        {
            missing.push(positional.name.to_string());
        }
    }
    for option in node.options {
        if option.required && !present.contains(option.long) {
            missing.push(format!("--{}", option.long));
        }
    }

    if missing.is_empty() {
        CommandReadiness::Ready
    } else {
        CommandReadiness::Incomplete {
            hint: format!("Missing {}.", missing.join(", ")),
            missing,
        }
    }
}

fn active_positional_for_readiness<'a>(analysis: &'a Analysis<'a>) -> Option<&'a PositionalSpec> {
    let token = analysis
        .parse
        .cursor_token
        .and_then(|index| analysis.parse.tokens.get(index))?;
    if analysis.parse.trailing_empty_token
        || analysis.parse.cursor != token.end
        || token.value.starts_with("--")
    {
        return None;
    }
    let node = analysis.node?;
    node.positionals.iter().find(|positional| {
        !analysis
            .operands
            .iter()
            .any(|(name, _)| name == positional.name && !positional.repeatable)
    })
}

#[derive(Debug)]
struct ReadinessScan<'a> {
    node: Option<&'a CommandNodeSpec>,
    path: Vec<String>,
    operands: Vec<(String, String)>,
    seen_options: Vec<String>,
    duplicate_options: Vec<String>,
    pending_option: Option<&'a OptionSpec>,
    unknown_option: Option<String>,
    unknown_command: Option<String>,
}

fn scan_for_readiness<'a>(analysis: &'a Analysis<'a>) -> ReadinessScan<'a> {
    if let Some(index) = analysis.parse.cursor_token {
        if let Some(token) = analysis.parse.tokens.get(index) {
            if !analysis.parse.trailing_empty_token && analysis.parse.cursor == token.end {
                let scan = scan_tokens(analysis.catalog, &analysis.parse.tokens[..=index]);
                if scan.unknown_command.is_none() && scan.unknown_option.is_none() {
                    return ReadinessScan {
                        node: scan.node,
                        path: scan.path,
                        operands: scan.operands,
                        seen_options: scan.seen_options,
                        duplicate_options: scan.duplicate_options,
                        pending_option: scan.pending_option,
                        unknown_option: scan.unknown_option,
                        unknown_command: scan.unknown_command,
                    };
                }
            }
        }
    }

    ReadinessScan {
        node: analysis.node,
        path: analysis.path.clone(),
        operands: analysis.operands.clone(),
        seen_options: analysis.seen_options.clone(),
        duplicate_options: analysis.duplicate_options.clone(),
        pending_option: analysis.pending_option,
        unknown_option: analysis.unknown_option.clone(),
        unknown_command: analysis.unknown_command.clone(),
    }
}

fn exact_meta_command(path: &[String], catalog: &CommandCatalog) -> bool {
    path.len() == 1
        && catalog
            .tree()
            .meta_commands
            .iter()
            .any(|meta| meta_name_matches(meta, &path[0]))
}

fn present_names(scan: &ReadinessScan<'_>) -> HashSet<String> {
    let mut present = scan.seen_options.iter().cloned().collect::<HashSet<_>>();
    for (name, _) in &scan.operands {
        present.insert(name.clone());
    }
    present
}

fn replacement_span(analysis: &Analysis<'_>) -> (usize, usize) {
    if let CompletionContextKind::OptionValue { .. } = analysis.parse.context {
        if let Some(index) = analysis.parse.cursor_token {
            if let Some(token) = analysis.parse.tokens.get(index) {
                if let Some((_, value_start)) = split_long_option_assignment(token) {
                    return (value_start, token.end);
                }
            }
        }
    }

    analysis
        .parse
        .cursor_token
        .and_then(|index| analysis.parse.tokens.get(index))
        .map(|token| (token.start, token.end))
        .unwrap_or((analysis.parse.cursor, analysis.parse.cursor))
}

fn task_candidate(item: TaskCompletionItem) -> CompletionCandidate {
    CompletionCandidate {
        replacement: item.id.to_string(),
        display: format!("{}  {}  \"{}\"", item.id, item.status.as_str(), item.title),
        detail: format!("task {}", item.status.as_str()),
        kind: CompletionKind::TaskId,
    }
}

fn ticket_candidate(item: TicketCompletionItem) -> CompletionCandidate {
    CompletionCandidate {
        replacement: item.id.to_string(),
        display: format!(
            "{}  {}  {}  \"{}\"",
            item.id,
            item.status.as_str(),
            item.run_id,
            item.summary
        ),
        detail: format!("ticket {} for {}", item.status.as_str(), item.task_id),
        kind: CompletionKind::TicketId,
    }
}

fn hint_candidate(display: &str, detail: &str) -> CompletionCandidate {
    CompletionCandidate {
        replacement: String::new(),
        display: display.to_string(),
        detail: detail.to_string(),
        kind: CompletionKind::Hint,
    }
}

fn value_hint(value: Option<&ValueSpec>, value_name: Option<&str>) -> CompletionCandidate {
    match value.map(|value| &value.source) {
        Some(ValueSource::HintOnly) => hint_candidate(
            value_name.unwrap_or("value"),
            "No provider-backed value completion.",
        ),
        Some(ValueSource::FilesystemPath) => hint_candidate(
            value_name.unwrap_or("path"),
            "Path completion is provided by the UI.",
        ),
        Some(ValueSource::NoCompletion) => {
            hint_candidate(value_name.unwrap_or("value"), "Enter a value.")
        }
        _ => hint_candidate(value_name.unwrap_or("value"), "No matching values."),
    }
}

fn option_detail(option: &OptionSpec) -> String {
    match option.value_name {
        Some(value_name) => format!(
            "{} <{}>",
            option.value.as_ref().map_or("", |value| value.help),
            value_name
        ),
        None => option
            .value
            .as_ref()
            .map_or(String::new(), |value| value.help.to_string()),
    }
}

fn redact_candidates(candidates: &mut [CompletionCandidate]) {
    for candidate in candidates {
        redact_candidate(candidate);
    }
}

fn redact_candidate(candidate: &mut CompletionCandidate) {
    let redactor = DefaultRedactor::new();
    candidate.display = redactor.redact(&candidate.display).text;
    candidate.detail = redactor.redact(&candidate.detail).text;
}

fn rank_candidates(candidates: &mut [CompletionCandidate]) {
    candidates.sort_by(|a, b| {
        rank_key(a)
            .cmp(&rank_key(b))
            .then_with(|| a.replacement.cmp(&b.replacement))
    });
}

fn rank_key(candidate: &CompletionCandidate) -> u8 {
    let text = format!("{} {}", candidate.display, candidate.detail);
    if text.contains("open")
        || text.contains("stuck")
        || text.contains("running")
        || text.contains("ready")
    {
        0
    } else {
        1
    }
}

fn longest_common_prefix(candidates: &[CompletionCandidate]) -> Option<String> {
    let first = candidates.first()?.replacement.clone();
    let prefix = candidates.iter().skip(1).fold(first, |prefix, candidate| {
        common_prefix(&prefix, &candidate.replacement)
    });
    (!prefix.is_empty()).then_some(prefix)
}

fn common_prefix(left: &str, right: &str) -> String {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .map(|(ch, _)| ch)
        .collect()
}

fn scoped_task_id(analysis: &Analysis<'_>) -> Option<TaskId> {
    analysis
        .operands
        .iter()
        .find(|(name, _)| name == "task-id")
        .and_then(|(_, value)| TaskId::parse(value.clone()).ok())
}

fn option_name_and_value(value: &str) -> (&str, Option<&str>) {
    let without_prefix = value.strip_prefix("--").unwrap_or(value);
    match without_prefix.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (without_prefix, None),
    }
}

fn split_long_option_assignment(token: &CompletionToken) -> Option<(&str, usize)> {
    let equal_offset = token.value.find('=')?;
    if !token.value.starts_with("--") {
        return None;
    }
    let name = &token.value[2..equal_offset];
    let raw_equal = token.raw.find('=')?;
    Some((name, token.start + raw_equal + 1))
}

fn push_seen_option(option: &OptionSpec, seen: &mut Vec<String>, duplicates: &mut Vec<String>) {
    if !option.repeatable && seen.iter().any(|existing| existing == option.long) {
        duplicates.push(option.long.to_string());
    }
    seen.push(option.long.to_string());
}

fn root_meta_matches(fragment: &str, catalog: &CommandCatalog) -> bool {
    !fragment.is_empty()
        && catalog.tree().meta_commands.iter().any(|meta| {
            meta.name.starts_with(fragment)
                || meta.aliases.iter().any(|alias| alias.starts_with(fragment))
        })
}

fn command_name_matches(node: &CommandNodeSpec, value: &str) -> bool {
    node.name == value || node.aliases.iter().any(|alias| alias == &value)
}

fn meta_name_matches(meta: &MetaCommandSpec, value: &str) -> bool {
    meta.name == value || meta.aliases.iter().any(|alias| alias == &value)
}

fn repo_identity(repo: Option<&PathBuf>) -> String {
    repo.map(|path| path.display().to_string())
        .unwrap_or_else(|| "<none>".to_string())
}

fn clamp_cursor(line: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(line.len());
    while cursor > 0 && !line.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HarnessError;
    use crate::runtime::build_cli;
    use std::cell::Cell;

    const TASK_READY: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TASK_STUCK: &str = "task_01BRZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_OPEN: &str = "ticket_01CRZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_OTHER: &str = "ticket_01DRZ3NDEKTSV4RRFFQ69G5FAV";
    const RUN_ID: &str = "run_01ERZ3NDEKTSV4RRFFQ69G5FAV";

    struct FakeStateView {
        fail_tasks: bool,
        fail_tickets: bool,
        task_queries: Cell<usize>,
        ticket_queries: Cell<usize>,
        last_ticket_scope: std::cell::RefCell<Option<TicketCompletionScope>>,
    }

    impl Default for FakeStateView {
        fn default() -> Self {
            Self {
                fail_tasks: false,
                fail_tickets: false,
                task_queries: Cell::new(0),
                ticket_queries: Cell::new(0),
                last_ticket_scope: std::cell::RefCell::new(None),
            }
        }
    }

    impl CompletionStateView for FakeStateView {
        fn tasks_for_completion(
            &self,
            _scope: TaskCompletionScope,
        ) -> HarnessResult<Vec<TaskCompletionItem>> {
            self.task_queries.set(self.task_queries.get() + 1);
            if self.fail_tasks {
                return Err(HarnessError::External("state unavailable".to_string()));
            }
            Ok(vec![
                TaskCompletionItem {
                    id: TaskId::parse(TASK_READY).unwrap(),
                    status: TaskStatus::Ready,
                    title: "Fix parser".to_string(),
                },
                TaskCompletionItem {
                    id: TaskId::parse(TASK_STUCK).unwrap(),
                    status: TaskStatus::Stuck,
                    title: "Password=super-secret-token-00000000000000000000".to_string(),
                },
            ])
        }

        fn tickets_for_completion(
            &self,
            scope: TicketCompletionScope,
        ) -> HarnessResult<Vec<TicketCompletionItem>> {
            self.ticket_queries.set(self.ticket_queries.get() + 1);
            self.last_ticket_scope.replace(Some(scope.clone()));
            if self.fail_tickets {
                return Err(HarnessError::External("state unavailable".to_string()));
            }
            let task_id = scope
                .task_id
                .unwrap_or_else(|| TaskId::parse(TASK_READY).unwrap());
            Ok(vec![
                TicketCompletionItem {
                    id: TicketId::parse(TICKET_OPEN).unwrap(),
                    task_id: task_id.clone(),
                    run_id: RunId::parse(RUN_ID).unwrap(),
                    status: TicketStatus::Open,
                    summary: "Need OPENAI_API_KEY=sk-proj-abcdefABCDEF1234567890abcdefABCDEF"
                        .to_string(),
                },
                TicketCompletionItem {
                    id: TicketId::parse(TICKET_OTHER).unwrap(),
                    task_id,
                    run_id: RunId::parse(RUN_ID).unwrap(),
                    status: TicketStatus::Resolved,
                    summary: "Resolved".to_string(),
                },
            ])
        }
    }

    fn test_context<'a>(
        catalog: &'a CommandCatalog,
        state: &'a FakeStateView,
    ) -> CompletionContext<'a> {
        CompletionContext {
            state,
            repo: None,
            catalog,
        }
    }

    #[test]
    fn completion_root_and_nested_command_completion() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let root = engine
            .complete("ta", 2, &test_context(&catalog, &state))
            .unwrap();
        assert!(
            root.candidates
                .iter()
                .any(|candidate| candidate.replacement == "task")
        );
        assert_eq!(root.replacement_start, 0);
        assert_eq!(root.replacement_end, 2);

        let nested = engine
            .complete("task r", 6, &test_context(&catalog, &state))
            .unwrap();
        assert_eq!(
            nested
                .candidates
                .iter()
                .map(|candidate| candidate.replacement.as_str())
                .collect::<Vec<_>>(),
            vec!["run"]
        );
    }

    #[test]
    fn completion_option_name_and_static_value_completion() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let options = engine
            .complete("doctor --p", 10, &test_context(&catalog, &state))
            .unwrap();
        assert_eq!(options.candidates[0].replacement, "--providers");

        let values = engine
            .complete("doctor --providers l", 20, &test_context(&catalog, &state))
            .unwrap();
        assert_eq!(values.candidates[0].replacement, "local");
        assert_eq!(values.replacement_start, 19);
        assert_eq!(values.replacement_end, 20);
    }

    #[test]
    fn completion_dynamic_task_and_ticket_ids_use_state_view() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let tasks = engine
            .complete("task get task_", 14, &test_context(&catalog, &state))
            .unwrap();
        assert_eq!(state.task_queries.get(), 1);
        assert!(
            tasks
                .candidates
                .iter()
                .all(|candidate| candidate.kind == CompletionKind::TaskId)
        );
        assert!(
            tasks
                .candidates
                .iter()
                .any(|candidate| candidate.replacement == TASK_READY)
        );

        let tickets = engine
            .complete("ticket get ticket_", 18, &test_context(&catalog, &state))
            .unwrap();
        assert_eq!(state.ticket_queries.get(), 1);
        assert!(
            tickets
                .candidates
                .iter()
                .all(|candidate| candidate.kind == CompletionKind::TicketId)
        );
        assert!(
            tickets
                .candidates
                .iter()
                .any(|candidate| candidate.replacement == TICKET_OPEN)
        );
    }

    #[test]
    fn completion_scopes_ticket_ids_to_resume_and_supervise_task() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::with_cache_capacity(0);

        engine
            .complete(
                &format!("resume {TASK_STUCK} --ticket ticket_"),
                68,
                &test_context(&catalog, &state),
            )
            .unwrap();
        assert_eq!(
            state
                .last_ticket_scope
                .borrow()
                .as_ref()
                .and_then(|scope| scope.task_id.as_ref())
                .map(TaskId::as_str),
            Some(TASK_STUCK)
        );

        engine
            .complete(
                &format!("supervise {TASK_READY} --ticket ticket_"),
                71,
                &test_context(&catalog, &state),
            )
            .unwrap();
        assert_eq!(
            state
                .last_ticket_scope
                .borrow()
                .as_ref()
                .and_then(|scope| scope.task_id.as_ref())
                .map(TaskId::as_str),
            Some(TASK_READY)
        );
    }

    #[test]
    fn completion_cursor_offsets_replacement_spans_and_flag_equals() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let middle = engine
            .complete("task get task_zzz", 13, &test_context(&catalog, &state))
            .unwrap();
        assert_eq!(middle.replacement_start, 9);
        assert_eq!(middle.replacement_end, 17);

        let equals = engine
            .complete("harness --output=j", 18, &test_context(&catalog, &state))
            .unwrap();
        assert_eq!(equals.replacement_start, 17);
        assert_eq!(equals.replacement_end, 18);
        assert_eq!(equals.candidates[0].replacement, "json");
    }

    #[test]
    fn completion_supports_globals_before_command_and_leading_harness() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let set = engine
            .complete(
                "harness --repo /tmp task l",
                26,
                &test_context(&catalog, &state),
            )
            .unwrap();
        assert_eq!(set.candidates[0].replacement, "list");
    }

    #[test]
    fn completion_reports_unterminated_quotes_without_failing() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let parse = engine.parse("run --title \"Fix", 16, &catalog);
        assert_eq!(parse.quote_state, QuoteState::Double);
        let set = engine
            .complete("run --title \"Fix", 16, &test_context(&catalog, &state))
            .unwrap();
        assert!(matches!(set.readiness, CommandReadiness::Incomplete { .. }));
    }

    #[test]
    fn completion_dynamic_failure_keeps_static_fallback_and_reports_status() {
        let catalog = build_cli();
        let state = FakeStateView {
            fail_tasks: true,
            ..FakeStateView::default()
        };
        let engine = CompletionEngine::new();

        let dynamic = engine
            .complete("task get task_", 14, &test_context(&catalog, &state))
            .unwrap();
        assert!(matches!(dynamic.status, CompletionStatus::Error(_)));
        assert!(dynamic.candidates.is_empty());

        let static_values = engine
            .complete("task list --status r", 20, &test_context(&catalog, &state))
            .unwrap();
        assert_eq!(static_values.status, CompletionStatus::Ready);
        assert_eq!(static_values.candidates[0].replacement, "ready");
    }

    #[test]
    fn completion_cache_returns_stale_rows_without_requerying_state() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let first = engine
            .complete("ticket get ticket_", 18, &test_context(&catalog, &state))
            .unwrap();
        assert_eq!(first.status, CompletionStatus::Ready);
        assert_eq!(state.ticket_queries.get(), 1);

        let second = engine
            .complete("ticket get ticket_", 18, &test_context(&catalog, &state))
            .unwrap();
        assert_eq!(second.status, CompletionStatus::Stale);
        assert_eq!(state.ticket_queries.get(), 1);
        assert!(
            second
                .candidates
                .iter()
                .all(|candidate| !candidate.display.contains("sk-proj-abcdef"))
        );
    }

    #[test]
    fn completion_returns_hint_rows_for_shell_escape_and_hint_only_values() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let shell = engine
            .complete("!cargo test", 11, &test_context(&catalog, &state))
            .unwrap();
        assert!(shell.candidates.is_empty());
        assert_eq!(shell.hint.as_ref().unwrap().kind, CompletionKind::Hint);

        let model = engine
            .complete(
                "ticket resolve ticket_01CRZ3NDEKTSV4RRFFQ69G5FAV --model ",
                61,
                &test_context(&catalog, &state),
            )
            .unwrap();
        assert!(model.candidates.is_empty());
        assert_eq!(model.hint.as_ref().unwrap().kind, CompletionKind::Hint);
    }

    #[test]
    fn completion_redacts_display_and_detail_text_but_not_replacement_ids() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let set = engine
            .complete("ticket get ticket_", 18, &test_context(&catalog, &state))
            .unwrap();
        let candidate = set
            .candidates
            .iter()
            .find(|candidate| candidate.replacement == TICKET_OPEN)
            .unwrap();
        assert_eq!(candidate.replacement, TICKET_OPEN);
        assert!(candidate.display.contains("[REDACTED"));
        assert!(!candidate.display.contains("sk-proj-abcdef"));
    }

    #[test]
    fn completion_readiness_reports_missing_and_conflicting_inputs() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let missing = engine
            .complete("supervise", 9, &test_context(&catalog, &state))
            .unwrap();
        assert!(matches!(
            missing.readiness,
            CommandReadiness::Incomplete { .. }
        ));

        let conflict = engine
            .complete(
                &format!("supervise --create {TASK_READY}"),
                65,
                &test_context(&catalog, &state),
            )
            .unwrap();
        assert!(matches!(
            conflict.readiness,
            CommandReadiness::Invalid { .. }
        ));
    }

    #[test]
    fn completion_readiness_includes_completed_cursor_token_values() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        let task_get_line = format!("task get {TASK_READY}");
        let task_get = engine
            .complete(
                &task_get_line,
                task_get_line.len(),
                &test_context(&catalog, &state),
            )
            .unwrap();
        assert_eq!(task_get.readiness, CommandReadiness::Ready);

        let resume_line = format!("resume {TASK_READY} --ticket {TICKET_OPEN}");
        let resume = engine
            .complete(
                &resume_line,
                resume_line.len(),
                &test_context(&catalog, &state),
            )
            .unwrap();
        assert_eq!(resume.readiness, CommandReadiness::Ready);
    }

    #[test]
    fn completion_readiness_marks_exact_meta_commands_ready() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        for command in ["exit", "quit", "help"] {
            let set = engine
                .complete(command, command.len(), &test_context(&catalog, &state))
                .unwrap();
            assert_eq!(set.readiness, CommandReadiness::Ready, "{command}");
        }
    }

    #[test]
    fn completion_fake_state_view_exposes_only_read_queries() {
        let catalog = build_cli();
        let state = FakeStateView::default();
        let engine = CompletionEngine::new();

        engine
            .complete("task run task_", 14, &test_context(&catalog, &state))
            .unwrap();
        engine
            .complete(
                "ticket resolve ticket_",
                22,
                &test_context(&catalog, &state),
            )
            .unwrap();

        assert_eq!(state.task_queries.get(), 1);
        assert_eq!(state.ticket_queries.get(), 1);
    }
}
