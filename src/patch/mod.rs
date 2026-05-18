use crate::{HarnessError, HarnessResult};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPatch {
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OllamaResponse {
    Patch(ParsedPatch),
    Stuck(StuckResponse),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StuckResponse {
    pub reason: String,
    pub question: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchValidationConfig {
    pub worktree_path: String,
    pub max_patch_bytes: u64,
    pub max_files_changed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchValidation {
    pub files_changed: Vec<String>,
    pub patch_bytes: u64,
    pub normalized_diff: String,
    pub apply_check: GitApplyInvocation,
    pub apply: GitApplyInvocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitApplyInvocation {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePatch {
    old_path: Option<String>,
    new_path: Option<String>,
    has_hunk: bool,
    new_file: bool,
    deleted: bool,
    renamed: bool,
    binary: bool,
    mode_only: bool,
    creation_index: bool,
    old_mode: Option<String>,
    new_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HunkHeader<'a> {
    old_start: u32,
    new_start: u32,
    suffix: &'a str,
}

pub fn parse_ollama_response(text: &str) -> HarnessResult<OllamaResponse> {
    let trimmed = text.trim();
    if trimmed.starts_with("```") {
        return parse_diff_response(trimmed).map(OllamaResponse::Patch);
    }
    if trimmed.starts_with("STUCK") {
        return parse_stuck_response(trimmed).map(OllamaResponse::Stuck);
    }
    Err(parse_error(
        "response must be exactly one diff fence or STUCK block",
    ))
}

pub fn parse_diff_response(trimmed: &str) -> HarnessResult<ParsedPatch> {
    let Some(body) = trimmed
        .strip_prefix("```diff\n")
        .and_then(|value| value.strip_suffix("\n```"))
    else {
        if trimmed.starts_with("```") && !trimmed.starts_with("```diff\n") {
            return Err(parse_error("patch response must use a diff fence"));
        }
        return Err(parse_error("patch response must be exactly one diff fence"));
    };

    if body.contains("```") {
        return Err(parse_error("nested fenced blocks are not allowed"));
    }
    if body.trim().is_empty() {
        return Err(parse_error("diff fence cannot be empty"));
    }

    Ok(ParsedPatch {
        diff: body.to_string(),
    })
}

pub fn parse_stuck_response(trimmed: &str) -> HarnessResult<StuckResponse> {
    let lines = trimmed.lines().collect::<Vec<_>>();
    if lines.len() == 1 {
        return parse_inline_stuck_response(trimmed);
    }
    if lines.len() != 3 {
        return Err(parse_error(
            "STUCK response must be one inline STUCK block or exactly three lines",
        ));
    }
    if lines[0] != "STUCK" {
        return Err(parse_error("STUCK response must start with STUCK"));
    }
    let reason = parse_stuck_field(lines[1], "reason")?;
    let question = parse_stuck_field(lines[2], "question")?;
    Ok(StuckResponse { reason, question })
}

fn parse_inline_stuck_response(line: &str) -> HarnessResult<StuckResponse> {
    let rest = line
        .strip_prefix("STUCK,")
        .or_else(|| line.strip_prefix("STUCK "))
        .ok_or_else(|| parse_error("inline STUCK response must start with STUCK,"))?
        .trim();
    let reason_start = rest
        .find("reason:")
        .ok_or_else(|| parse_error("missing STUCK field reason"))?;
    let after_reason = rest[reason_start + "reason:".len()..].trim();
    let (reason, question) = if let Some(question_start) = after_reason.find("question:") {
        let reason = after_reason[..question_start]
            .trim()
            .trim_end_matches(',')
            .trim();
        let question = after_reason[question_start + "question:".len()..]
            .trim()
            .trim_start_matches(',')
            .trim();
        (reason, question)
    } else {
        (
            after_reason.trim_end_matches(',').trim(),
            "How should this task continue?",
        )
    };
    validate_stuck_value(reason, "reason")?;
    validate_stuck_value(question, "question")?;
    Ok(StuckResponse {
        reason: reason.to_string(),
        question: question.to_string(),
    })
}

pub fn validate_patch_safety(
    diff: &str,
    config: &PatchValidationConfig,
) -> HarnessResult<PatchValidation> {
    let patch_bytes = diff.len() as u64;
    if patch_bytes > config.max_patch_bytes {
        return Err(security_error(format!(
            "patch is {patch_bytes} bytes and exceeds {} byte limit",
            config.max_patch_bytes
        )));
    }

    let worktree = fs::canonicalize(&config.worktree_path).map_err(|error| {
        HarnessError::External(format!(
            "failed to canonicalize worktree {}: {error}",
            config.worktree_path
        ))
    })?;
    let file_patches = parse_file_patches(diff)?;
    if file_patches.is_empty() {
        return Err(security_error("patch contains no file changes"));
    }

    let mut changed = BTreeSet::new();
    for file_patch in &file_patches {
        validate_file_patch(file_patch, &worktree)?;
        if let Some(path) = &file_patch.new_path {
            changed.insert(path.clone());
        }
    }

    if changed.len() > config.max_files_changed as usize {
        return Err(security_error(format!(
            "patch changes {} files and exceeds {} file limit",
            changed.len(),
            config.max_files_changed
        )));
    }

    let files_changed = changed.into_iter().collect::<Vec<_>>();
    let normalized_diff = normalize_hunk_counts(diff);
    Ok(PatchValidation {
        files_changed,
        patch_bytes,
        normalized_diff,
        apply_check: GitApplyInvocation {
            program: "git".to_string(),
            args: vec![
                "apply".to_string(),
                "--check".to_string(),
                "--recount".to_string(),
                "-".to_string(),
            ],
            cwd: worktree.to_string_lossy().into_owned(),
        },
        apply: GitApplyInvocation {
            program: "git".to_string(),
            args: vec![
                "apply".to_string(),
                "--recount".to_string(),
                "-".to_string(),
            ],
            cwd: worktree.to_string_lossy().into_owned(),
        },
    })
}

pub fn normalize_hunk_counts(diff: &str) -> String {
    let diff = normalize_new_file_headers(diff);
    let lines = diff.lines().collect::<Vec<_>>();
    let mut normalized = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let Some(header) = parse_hunk_header(lines[index]) else {
            normalized.push(lines[index].to_string());
            index += 1;
            continue;
        };

        let mut old_count = 0_u32;
        let mut new_count = 0_u32;
        let mut body = Vec::new();
        index += 1;
        while index < lines.len()
            && !lines[index].starts_with("diff --git ")
            && parse_hunk_header(lines[index]).is_none()
        {
            let line = lines[index];
            match line.as_bytes().first().copied() {
                Some(b' ') => {
                    old_count += 1;
                    new_count += 1;
                }
                Some(b'-') => old_count += 1,
                Some(b'+') => new_count += 1,
                _ => {}
            }
            body.push(line.to_string());
            index += 1;
        }

        normalized.push(format!(
            "@@ -{},{} +{},{} @@{}",
            header.old_start, old_count, header.new_start, new_count, header.suffix
        ));
        normalized.extend(body);
    }

    let mut output = normalized.join("\n");
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn normalize_new_file_headers(diff: &str) -> String {
    let lines = diff.lines().collect::<Vec<_>>();
    let mut normalized = Vec::with_capacity(lines.len());
    let mut index = 0;

    while index < lines.len() {
        if !lines[index].starts_with("diff --git ") {
            normalized.push(lines[index].to_string());
            index += 1;
            continue;
        }

        let start = index;
        index += 1;
        while index < lines.len() && !lines[index].starts_with("diff --git ") {
            index += 1;
        }
        let section = &lines[start..index];
        let is_new_file = section.iter().any(|line| *line == "--- /dev/null")
            || section
                .iter()
                .any(|line| line.starts_with("index 0000000.."));
        let has_new_file_mode = section
            .iter()
            .any(|line| line.starts_with("new file mode "));

        normalized.push(section[0].to_string());
        if is_new_file && !has_new_file_mode {
            normalized.push("new file mode 100644".to_string());
        }
        normalized.extend(section.iter().skip(1).map(|line| {
            if is_new_file && line.starts_with("--- ") && *line != "--- /dev/null" {
                "--- /dev/null".to_string()
            } else {
                line.to_string()
            }
        }));
    }

    normalized.join("\n")
}

fn parse_stuck_field(line: &str, field: &'static str) -> HarnessResult<String> {
    let prefix = format!("{field}: ");
    let Some(value) = line.strip_prefix(&prefix) else {
        return Err(parse_error(format!("missing STUCK field {field}")));
    };
    validate_stuck_value(value, field)?;
    Ok(value.to_string())
}

fn validate_stuck_value(value: &str, field: &'static str) -> HarnessResult<()> {
    if value.is_empty() {
        return Err(parse_error(format!("STUCK field {field} cannot be empty")));
    }
    if value.len() > 1000 {
        return Err(parse_error(format!(
            "STUCK field {field} exceeds 1000 chars"
        )));
    }
    Ok(())
}

fn parse_file_patches(diff: &str) -> HarnessResult<Vec<FilePatch>> {
    let mut files = Vec::new();
    let mut current: Option<FilePatch> = None;

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(file) = current.take() {
                files.push(finalize_file_patch(file)?);
            }
            let (old_path, new_path) = parse_diff_git_paths(rest)?;
            current = Some(FilePatch {
                old_path: Some(old_path),
                new_path: Some(new_path),
                has_hunk: false,
                new_file: false,
                deleted: false,
                renamed: false,
                binary: false,
                mode_only: false,
                creation_index: false,
                old_mode: None,
                new_mode: None,
            });
            continue;
        }

        let Some(file) = current.as_mut() else {
            if line.trim().is_empty() {
                continue;
            }
            return Err(parse_error("patch must start with diff --git"));
        };

        match line {
            value if value.starts_with("new file mode ") => {
                file.new_file = true;
                file.new_mode = value.strip_prefix("new file mode ").map(str::to_string);
            }
            value if value.starts_with("deleted file mode ") => {
                file.deleted = true;
                file.old_mode = value.strip_prefix("deleted file mode ").map(str::to_string);
            }
            value if value.starts_with("old mode ") => {
                file.mode_only = true;
                file.old_mode = value.strip_prefix("old mode ").map(str::to_string);
            }
            value if value.starts_with("new mode ") => {
                file.mode_only = true;
                file.new_mode = value.strip_prefix("new mode ").map(str::to_string);
            }
            value if value.starts_with("rename from ") || value.starts_with("rename to ") => {
                file.renamed = true;
            }
            value
                if value.starts_with("Binary files ") || value.starts_with("GIT binary patch") =>
            {
                file.binary = true;
            }
            value if value.starts_with("index 0000000..") => {
                file.creation_index = true;
            }
            value if value.starts_with("--- ") => {
                file.old_path = parse_patch_side_path(value.strip_prefix("--- ").unwrap(), "a/")?;
                if file.old_path.is_none() {
                    file.new_file = true;
                }
            }
            value if value.starts_with("+++ ") => {
                file.new_path = parse_patch_side_path(value.strip_prefix("+++ ").unwrap(), "b/")?;
                if file.new_path.is_none() {
                    file.deleted = true;
                }
            }
            value if value.starts_with("@@") => file.has_hunk = true,
            _ => {}
        }
    }

    if let Some(file) = current.take() {
        files.push(finalize_file_patch(file)?);
    }

    Ok(files)
}

fn finalize_file_patch(mut file: FilePatch) -> HarnessResult<FilePatch> {
    if file.creation_index && !file.deleted {
        file.old_path = None;
        file.new_file = true;
    }
    if file.binary {
        return Ok(file);
    }
    if file.mode_only && !file.has_hunk {
        return Ok(file);
    }
    if file.new_path.is_none() && !file.deleted {
        return Err(parse_error("file patch is missing +++ path"));
    }
    Ok(file)
}

fn parse_diff_git_paths(rest: &str) -> HarnessResult<(String, String)> {
    let (old_token, remaining) = parse_git_path_token(rest)?;
    let remaining = remaining.trim_start();
    let (new_token, trailing) = parse_git_path_token(remaining)?;
    if !trailing.trim().is_empty() {
        return Err(parse_error("diff --git line has trailing content"));
    }
    let old_path = old_token
        .strip_prefix("a/")
        .ok_or_else(|| parse_error("diff old path must start with a/"))?;
    let new_path = new_token
        .strip_prefix("b/")
        .ok_or_else(|| parse_error("diff new path must start with b/"))?;
    Ok((old_path.to_string(), new_path.to_string()))
}

fn parse_patch_side_path(value: &str, prefix: &str) -> HarnessResult<Option<String>> {
    if value == "/dev/null" {
        return Ok(None);
    }
    let (token, _) = parse_git_path_token(value)?;
    let path = token
        .strip_prefix(prefix)
        .ok_or_else(|| parse_error(format!("patch side path must start with {prefix}")))?;
    Ok(Some(path.to_string()))
}

fn parse_git_path_token(input: &str) -> HarnessResult<(String, &str)> {
    let input = input.trim_start();
    if input.is_empty() {
        return Err(parse_error("missing git path token"));
    }

    if let Some(rest) = input.strip_prefix('"') {
        return parse_quoted_git_path(rest);
    }

    let end = input
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(input.len());
    if end == 0 {
        return Err(parse_error("missing git path token"));
    }
    Ok((input[..end].to_string(), &input[end..]))
}

fn parse_quoted_git_path(input: &str) -> HarnessResult<(String, &str)> {
    let mut output = String::new();
    let mut chars = input.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '"' => return Ok((output, &input[index + ch.len_utf8()..])),
            '\\' => {
                let Some((_, escaped)) = chars.next() else {
                    return Err(parse_error("unterminated quoted git path escape"));
                };
                match escaped {
                    '\\' => output.push('\\'),
                    '"' => output.push('"'),
                    'n' => output.push('\n'),
                    'r' => output.push('\r'),
                    't' => output.push('\t'),
                    value if value.is_ascii_digit() && value < '8' => {
                        let mut octal = value.to_string();
                        while octal.len() < 3 {
                            let Some((_, next)) = chars.peek().copied() else {
                                break;
                            };
                            if !next.is_ascii_digit() || next >= '8' {
                                break;
                            }
                            octal.push(next);
                            chars.next();
                        }
                        let byte = u8::from_str_radix(&octal, 8)
                            .map_err(|_| parse_error("invalid quoted git path octal escape"))?;
                        output.push(byte as char);
                    }
                    other => output.push(other),
                }
            }
            other => output.push(other),
        }
    }

    Err(parse_error("unterminated quoted git path"))
}

fn parse_hunk_header(line: &str) -> Option<HunkHeader<'_>> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, suffix) = rest.split_once("@@")?;
    let mut parts = ranges.split_whitespace();
    let old_range = parts.next()?;
    let new_range = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some(HunkHeader {
        old_start: parse_hunk_start(old_range, '-')?,
        new_start: parse_hunk_start(new_range, '+')?,
        suffix,
    })
}

fn parse_hunk_start(range: &str, sign: char) -> Option<u32> {
    let value = range.strip_prefix(sign)?;
    let start = value.split_once(',').map_or(value, |(start, _)| start);
    start.parse().ok()
}

fn validate_file_patch(file: &FilePatch, worktree: &Path) -> HarnessResult<()> {
    if file.binary {
        return Err(security_error("binary patches are not allowed"));
    }
    if file.renamed {
        return Err(security_error("renames are not allowed"));
    }
    if file.deleted {
        return Err(security_error("deletes are not allowed"));
    }
    if file.mode_only && !file.has_hunk {
        return Err(security_error("mode-only patches are not allowed"));
    }
    if !file.has_hunk {
        return Err(security_error("patch file has no content hunks"));
    }
    if is_special_mode(file.old_mode.as_deref()) || is_special_mode(file.new_mode.as_deref()) {
        return Err(security_error(
            "symlink and submodule mode changes are not allowed",
        ));
    }

    let new_path = file
        .new_path
        .as_ref()
        .ok_or_else(|| security_error("patch is missing new path"))?;
    validate_relative_git_path(new_path)?;

    if let Some(old_path) = &file.old_path {
        validate_relative_git_path(old_path)?;
        if !file.new_file && old_path != new_path {
            return Err(security_error(
                "path changes are treated as renames and are not allowed",
            ));
        }
    }

    ensure_path_stays_in_worktree(worktree, new_path, file.new_file)?;
    if let Some(old_path) = &file.old_path {
        ensure_path_stays_in_worktree(worktree, old_path, false)?;
    }

    Ok(())
}

fn validate_relative_git_path(path: &str) -> HarnessResult<()> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err(security_error(format!(
            "absolute patch path rejected: {path}"
        )));
    }
    for component in candidate.components() {
        match component {
            Component::ParentDir => {
                return Err(security_error(format!("path traversal rejected: {path}")));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(security_error(format!(
                    "absolute patch path rejected: {path}"
                )));
            }
            _ => {}
        }
    }

    let mut components = candidate.components();
    match components.next() {
        Some(Component::Normal(first)) if first == ".git" => {
            return Err(security_error(".git paths are not allowed"));
        }
        Some(Component::Normal(first)) if first == ".harness" => {
            return Err(security_error(".harness paths are not allowed"));
        }
        _ => {}
    }

    if path == ".gitconfig" || path.ends_with("/.gitconfig") {
        return Err(security_error("git config edits are not allowed"));
    }
    if path.starts_with(".git/hooks/") {
        return Err(security_error("git hook edits are not allowed"));
    }

    Ok(())
}

fn ensure_path_stays_in_worktree(
    worktree: &Path,
    relative: &str,
    new_file: bool,
) -> HarnessResult<()> {
    let full_path = worktree.join(relative);
    let canonical_target = if full_path.exists() {
        fs::canonicalize(&full_path).map_err(|error| {
            HarnessError::External(format!(
                "failed to canonicalize patch path {relative}: {error}"
            ))
        })?
    } else if new_file {
        canonicalize_existing_parent(&full_path)?
    } else {
        return Err(security_error(format!(
            "modification target does not exist as a normal file: {relative}"
        )));
    };

    if !canonical_target.starts_with(worktree) {
        return Err(security_error(format!(
            "patch path escapes worktree after canonicalization: {relative}"
        )));
    }

    if full_path.exists() {
        let metadata = fs::symlink_metadata(&full_path).map_err(|error| {
            HarnessError::External(format!("failed to read metadata for {relative}: {error}"))
        })?;
        if !metadata.file_type().is_file() {
            return Err(security_error(format!(
                "patch target is not a normal file: {relative}"
            )));
        }
    }

    Ok(())
}

fn canonicalize_existing_parent(full_path: &Path) -> HarnessResult<PathBuf> {
    let mut current = full_path.parent();
    while let Some(parent) = current {
        if parent.exists() {
            return fs::canonicalize(parent).map_err(|error| {
                HarnessError::External(format!(
                    "failed to canonicalize patch parent {}: {error}",
                    parent.display()
                ))
            });
        }
        current = parent.parent();
    }
    Err(security_error(format!(
        "patch path {} has no existing parent",
        full_path.display()
    )))
}

fn is_special_mode(mode: Option<&str>) -> bool {
    matches!(mode, Some(value) if value.starts_with("120000") || value.starts_with("160000"))
}

fn parse_error(message: impl Into<String>) -> HarnessError {
    HarnessError::Usage(format!("invalid model response: {}", message.into()))
}

fn security_error(message: impl Into<String>) -> HarnessError {
    HarnessError::SecurityPolicy(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn patch_parser_accepts_exact_diff_fence() {
        let response = parse_ollama_response(
            "```diff\ndiff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n```",
        )
        .unwrap();
        assert_eq!(
            response,
            OllamaResponse::Patch(ParsedPatch {
                diff: "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b".to_string()
            })
        );
    }

    #[test]
    fn patch_parser_accepts_exact_stuck_block() {
        let response = parse_ollama_response(
            "STUCK\nreason: dependency is missing\nquestion: Which crate should I use?",
        )
        .unwrap();
        assert_eq!(
            response,
            OllamaResponse::Stuck(StuckResponse {
                reason: "dependency is missing".to_string(),
                question: "Which crate should I use?".to_string()
            })
        );
    }

    #[test]
    fn patch_parser_accepts_documented_inline_stuck_block() {
        let response = parse_ollama_response(
            "STUCK, reason: no existing crate, question: should I create Cargo.toml?",
        )
        .unwrap();
        assert_eq!(
            response,
            OllamaResponse::Stuck(StuckResponse {
                reason: "no existing crate".to_string(),
                question: "should I create Cargo.toml?".to_string()
            })
        );

        let response = parse_ollama_response("STUCK, reason: no existing crate").unwrap();
        assert_eq!(
            response,
            OllamaResponse::Stuck(StuckResponse {
                reason: "no existing crate".to_string(),
                question: "How should this task continue?".to_string()
            })
        );
    }

    #[test]
    fn patch_parser_rejects_prose_multiple_nested_and_non_diff_fences() {
        assert!(parse_ollama_response("Here:\n```diff\nx\n```").is_err());
        assert!(parse_ollama_response("```diff\nx\n```\nmore").is_err());
        assert!(parse_ollama_response("```diff\n```text\nx\n```\n```").is_err());
        assert!(parse_ollama_response("```text\nx\n```").is_err());
        assert!(parse_ollama_response("```diff\nx\n```\n```diff\ny\n```").is_err());
    }

    #[test]
    fn patch_parser_rejects_missing_or_multiline_stuck_fields() {
        assert!(parse_ollama_response("STUCK\nreason: no").is_err());
        assert!(parse_ollama_response("STUCK\nreason: no\nquestion: q\nextra").is_err());
        assert!(parse_ollama_response("STUCK\nquestion: q\nreason: r").is_err());
        assert!(
            parse_ollama_response(&format!("STUCK\nreason: {}\nquestion: q", "x".repeat(1001)))
                .is_err()
        );
    }

    #[test]
    fn patch_safety_allows_existing_file_modification_and_new_file() {
        let temp = tempfile::tempdir().unwrap();
        write_file(temp.path().join("src.rs"), "old\n");
        let diff = "diff --git a/src.rs b/src.rs\n--- a/src.rs\n+++ b/src.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/new.rs b/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+new\n";

        let validation = validate_patch_safety(diff, &config(temp.path())).unwrap();
        assert_eq!(validation.files_changed, vec!["new.rs", "src.rs"]);
        assert_eq!(
            validation.apply_check.args,
            ["apply", "--check", "--recount", "-"]
        );
        assert_eq!(validation.apply.args, ["apply", "--recount", "-"]);
    }

    #[test]
    fn patch_safety_allows_new_file_inside_new_directory() {
        let temp = tempfile::tempdir().unwrap();
        let diff = "diff --git a/src/main.rs b/src/main.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/main.rs\n@@ -0,0 +1,3 @@\n+fn main() {\n+    println!(\"Hello, world!\");\n+}\n";

        let validation = validate_patch_safety(diff, &config(temp.path())).unwrap();

        assert_eq!(validation.files_changed, vec!["src/main.rs"]);
    }

    #[test]
    fn patch_safety_normalizes_generated_hunk_counts() {
        let temp = tempfile::tempdir().unwrap();
        let diff = "diff --git a/Cargo.toml b/Cargo.toml\nnew file mode 100644\n--- /dev/null\n+++ b/Cargo.toml\n@@ -0,0 +1,8 @@\n+[package]\n+name = \"hello_world\"\n+version = \"0.1.0\"\n+edition = \"2021\"\n+\n+[dependencies]\n+\n+[profile.release]\n+opt-level = 3\ndiff --git a/src/main.rs b/src/main.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/main.rs\n@@ -0,0 +1,3 @@\n+fn main() {\n+    println!(\"Hello, world!\");\n+}\n";

        let validation = validate_patch_safety(diff, &config(temp.path())).unwrap();

        assert!(validation.normalized_diff.contains("@@ -0,0 +1,9 @@"));
        assert!(validation.normalized_diff.contains("@@ -0,0 +1,3 @@"));
        assert!(validation.normalized_diff.ends_with('\n'));
    }

    #[test]
    fn patch_safety_adds_omitted_new_file_mode_for_dev_null_creation() {
        let temp = tempfile::tempdir().unwrap();
        let diff = "diff --git a/Cargo.toml b/Cargo.toml\nindex 0000000..1234567 100644\n--- /dev/null\n+++ b/Cargo.toml\n@@ -0,0 +1,3 @@\n+[package]\n+name = \"hello_world\"\n+version = \"0.1.0\"\n";

        let validation = validate_patch_safety(diff, &config(temp.path())).unwrap();

        assert!(
            validation
                .normalized_diff
                .contains("diff --git a/Cargo.toml b/Cargo.toml\nnew file mode 100644\nindex")
        );
    }

    #[test]
    fn patch_safety_treats_zero_index_patch_as_new_file_creation() {
        let temp = tempfile::tempdir().unwrap();
        let diff = "diff --git a/Cargo.toml b/Cargo.toml\nindex 0000000..1234567 100644\n--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -0,0 +1,3 @@\n+[package]\n+name = \"hello_world\"\n+version = \"0.1.0\"\n";

        let validation = validate_patch_safety(diff, &config(temp.path())).unwrap();

        assert!(validation.normalized_diff.contains(
            "diff --git a/Cargo.toml b/Cargo.toml\nnew file mode 100644\nindex 0000000..1234567 100644\n--- /dev/null"
        ));
    }

    #[test]
    fn patch_safety_accepts_git_quoted_paths_with_spaces() {
        let temp = tempfile::tempdir().unwrap();
        write_file(temp.path().join("src file.rs"), "old\n");
        let diff = "diff --git \"a/src file.rs\" \"b/src file.rs\"\n--- \"a/src file.rs\"\n+++ \"b/src file.rs\"\n@@ -1 +1 @@\n-old\n+new\n";

        let validation = validate_patch_safety(diff, &config(temp.path())).unwrap();
        assert_eq!(validation.files_changed, vec!["src file.rs"]);
    }

    #[test]
    fn patch_safety_rejects_traversal_absolute_internal_git_and_harness_paths() {
        let temp = tempfile::tempdir().unwrap();
        write_file(temp.path().join("safe.rs"), "old\n");
        assert_rejects(
            temp.path(),
            "diff --git a/../x b/../x\n--- a/../x\n+++ b/../x\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert_rejects(
            temp.path(),
            "diff --git a//tmp/x b//tmp/x\n--- a//tmp/x\n+++ b//tmp/x\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert_rejects(
            temp.path(),
            "diff --git a/.git/hooks/pre-commit b/.git/hooks/pre-commit\n--- a/.git/hooks/pre-commit\n+++ b/.git/hooks/pre-commit\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert_rejects(
            temp.path(),
            "diff --git a/.harness/state.sqlite b/.harness/state.sqlite\n--- a/.harness/state.sqlite\n+++ b/.harness/state.sqlite\n@@ -1 +1 @@\n-a\n+b\n",
        );
    }

    #[test]
    fn patch_safety_rejects_symlink_escape() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write_file(outside.path().join("target.rs"), "old\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path().join("target.rs"),
            temp.path().join("link.rs"),
        )
        .unwrap();

        #[cfg(unix)]
        assert_rejects(
            temp.path(),
            "diff --git a/link.rs b/link.rs\n--- a/link.rs\n+++ b/link.rs\n@@ -1 +1 @@\n-old\n+new\n",
        );
    }

    #[test]
    fn patch_safety_rejects_binary_mode_rename_delete_and_special_modes() {
        let temp = tempfile::tempdir().unwrap();
        write_file(temp.path().join("a"), "old\n");
        write_file(temp.path().join("b"), "old\n");
        assert_rejects(
            temp.path(),
            "diff --git a/a b/a\nBinary files a/a and b/a differ\n",
        );
        assert_rejects(
            temp.path(),
            "diff --git a/a b/a\nold mode 100644\nnew mode 100755\n",
        );
        assert_rejects(
            temp.path(),
            "diff --git a/a b/b\nsimilarity index 100%\nrename from a\nrename to b\n",
        );
        assert_rejects(
            temp.path(),
            "diff --git a/a b/a\ndeleted file mode 100644\n--- a/a\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n",
        );
        assert_rejects(
            temp.path(),
            "diff --git a/a b/a\nold mode 120000\nnew mode 120000\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
        );
        assert_rejects(
            temp.path(),
            "diff --git a/a b/a\nold mode 160000\nnew mode 160000\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
        );
    }

    #[test]
    fn patch_safety_rejects_oversized_patches_and_too_many_files() {
        let temp = tempfile::tempdir().unwrap();
        write_file(temp.path().join("a"), "old\n");
        write_file(temp.path().join("b"), "old\n");
        let diff = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n";
        let mut small = config(temp.path());
        small.max_patch_bytes = 4;
        assert!(validate_patch_safety(diff, &small).is_err());

        let many = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1 @@\n-old\n+new\n";
        let mut limited = config(temp.path());
        limited.max_files_changed = 1;
        assert!(validate_patch_safety(many, &limited).is_err());
    }

    fn assert_rejects(worktree: &Path, diff: &str) {
        assert!(
            validate_patch_safety(diff, &config(worktree)).is_err(),
            "expected rejection for diff:\n{diff}"
        );
    }

    fn config(worktree: &Path) -> PatchValidationConfig {
        PatchValidationConfig {
            worktree_path: worktree.to_string_lossy().into_owned(),
            max_patch_bytes: 1024 * 1024,
            max_files_changed: 20,
        }
    }

    fn write_file(path: impl AsRef<Path>, body: &str) {
        if let Some(parent) = path.as_ref().parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        file.write_all(body.as_bytes()).unwrap();
    }
}
