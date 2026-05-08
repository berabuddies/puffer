use crate::workspace_paths;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_HEAD_LIMIT: usize = 250;
const VCS_DIRECTORIES_TO_EXCLUDE: [&str; 6] = [".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

/// Linux pseudo-filesystem directories that GNU grep would otherwise
/// recurse into when the agent passes `path: "/"` (or any ancestor).
/// They contain millions of synthetic entries — most unreadable —
/// and produce gigabytes of stderr like
/// `grep: /proc/sys/kernel/apparmor_display_secid_mode: …`. Observed
/// 2026-04-12 in `make-doom-for-mips` step 44 where a single `Grep`
/// over `/` produced 1.8MB of stderr that puffer correctly truncated
/// to a 2KB preview, leaving the agent unable to act on the result.
///
/// Applied as `--exclude-dir=<basename>` only when the grep target
/// is `/` exactly. Picking that narrow trigger avoids surprising
/// behavior on user projects that happen to contain a sub-directory
/// named `proc` (rare but legal).
const PSEUDO_FS_DIRS: [&str; 4] = ["proc", "sys", "dev", "run"];

#[derive(Debug, Deserialize)]
struct ClaudeGrepInput {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    output_mode: Option<String>,
    #[serde(default, rename = "-B")]
    before_context: Option<usize>,
    #[serde(default, rename = "-A")]
    after_context: Option<usize>,
    #[serde(default, rename = "-C")]
    context_short: Option<usize>,
    #[serde(default)]
    context: Option<usize>,
    #[serde(default, rename = "-n")]
    show_line_numbers: Option<bool>,
    #[serde(default, rename = "-i")]
    case_insensitive: Option<bool>,
    #[serde(default, rename = "type")]
    file_type: Option<String>,
    #[serde(default)]
    head_limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    multiline: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrepMode {
    Content,
    FilesWithMatches,
    Count,
}

/// Executes the Claude-compatible `Grep` tool over the current workspace.
///
/// Input fields mirror Claude Code's schema, including richer fields like
/// `output_mode`, `glob`, `type`, context flags (`-A`, `-B`, `-C`, `context`),
/// pagination (`head_limit`, `offset`), and multiline search.
///
/// The output is returned as JSON with Claude-like fields:
/// `mode`, `numFiles`, `filenames`, and optional `content`, `numLines`,
/// `numMatches`, `appliedLimit`, and `appliedOffset`.
pub fn execute_claude_grep(
    cwd: &Path,
    working_dirs: &[PathBuf],
    allow_all_paths: bool,
    input: Value,
) -> Result<String> {
    let input: ClaudeGrepInput = serde_json::from_value(input).context("invalid Grep input")?;
    if input.pattern.trim().is_empty() {
        bail!("Grep pattern cannot be empty");
    }

    let mode = parse_mode(input.output_mode.as_deref())?;
    let sandbox_mode = if allow_all_paths {
        "danger-full-access"
    } else {
        "workspace-write"
    };
    let absolute_target = input
        .path
        .as_deref()
        .map(|path| {
            workspace_paths::resolve_path_for_session(
                cwd,
                working_dirs,
                sandbox_mode,
                Path::new(path),
            )
        })
        .transpose()?
        .unwrap_or_else(|| cwd.to_path_buf());
    if !absolute_target.exists() {
        bail!("Path does not exist: {}", absolute_target.display());
    }

    let mut args = vec![
        "--hidden".to_string(),
        "--max-columns".to_string(),
        "500".to_string(),
    ];
    append_vcs_exclusions(&mut args);
    append_pseudo_fs_exclusions_rg(&mut args, &absolute_target);
    if input.multiline.unwrap_or(false) {
        args.push("-U".to_string());
        args.push("--multiline-dotall".to_string());
    }
    if input.case_insensitive.unwrap_or(false) {
        args.push("-i".to_string());
    }
    append_mode_flags(&mut args, mode);
    append_context_flags(&mut args, mode, &input);
    append_pattern_arg(&mut args, &input.pattern);
    append_optional_type(&mut args, input.file_type.as_deref());
    append_glob_args(&mut args, input.glob.as_deref());
    args.push("--".to_string());
    args.push(absolute_target.to_string_lossy().to_string());

    let raw_lines = if rg_available() {
        run_ripgrep(cwd, &args)?
    } else {
        run_grep_fallback(cwd, &absolute_target, &input, mode)?
    };
    let offset = input.offset.unwrap_or(0);
    let (limit_applied, entries) = match mode {
        GrepMode::FilesWithMatches => {
            let sorted = sort_paths_by_mtime_desc(cwd, raw_lines);
            apply_head_limit(&sorted, input.head_limit, offset)
        }
        GrepMode::Content | GrepMode::Count => {
            apply_head_limit(&raw_lines, input.head_limit, offset)
        }
    };

    let output = match mode {
        GrepMode::Content => content_mode_output(cwd, entries, limit_applied, offset),
        GrepMode::FilesWithMatches => files_mode_output(cwd, entries, limit_applied, offset),
        GrepMode::Count => count_mode_output(cwd, entries, limit_applied, offset),
    };

    Ok(serde_json::to_string_pretty(&output)?)
}

fn parse_mode(mode: Option<&str>) -> Result<GrepMode> {
    match mode.unwrap_or("files_with_matches") {
        "content" => Ok(GrepMode::Content),
        "files_with_matches" => Ok(GrepMode::FilesWithMatches),
        "count" => Ok(GrepMode::Count),
        other => bail!("unsupported Grep output_mode `{other}`"),
    }
}

fn append_vcs_exclusions(args: &mut Vec<String>) {
    for dir in VCS_DIRECTORIES_TO_EXCLUDE {
        args.push("--glob".to_string());
        args.push(format!("!{dir}"));
    }
}

fn append_mode_flags(args: &mut Vec<String>, mode: GrepMode) {
    match mode {
        GrepMode::FilesWithMatches => args.push("-l".to_string()),
        GrepMode::Count => args.push("-c".to_string()),
        GrepMode::Content => {}
    }
}

fn append_context_flags(args: &mut Vec<String>, mode: GrepMode, input: &ClaudeGrepInput) {
    if mode != GrepMode::Content {
        return;
    }

    if input.show_line_numbers.unwrap_or(true) {
        args.push("-n".to_string());
    }

    if let Some(value) = input.context.or(input.context_short) {
        args.push("-C".to_string());
        args.push(value.to_string());
        return;
    }
    if let Some(value) = input.before_context {
        args.push("-B".to_string());
        args.push(value.to_string());
    }
    if let Some(value) = input.after_context {
        args.push("-A".to_string());
        args.push(value.to_string());
    }
}

fn append_pattern_arg(args: &mut Vec<String>, pattern: &str) {
    if pattern.starts_with('-') {
        args.push("-e".to_string());
        args.push(pattern.to_string());
    } else {
        args.push(pattern.to_string());
    }
}

fn append_optional_type(args: &mut Vec<String>, file_type: Option<&str>) {
    if let Some(file_type) = file_type.filter(|value| !value.trim().is_empty()) {
        args.push("--type".to_string());
        args.push(file_type.to_string());
    }
}

fn append_glob_args(args: &mut Vec<String>, glob_value: Option<&str>) {
    let Some(glob_value) = glob_value.filter(|value| !value.trim().is_empty()) else {
        return;
    };

    for token in glob_value.split_whitespace() {
        if token.contains('{') && token.contains('}') {
            args.push("--glob".to_string());
            args.push(token.to_string());
            continue;
        }
        for part in token
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            args.push("--glob".to_string());
            args.push(part.to_string());
        }
    }
}

fn run_ripgrep(cwd: &Path, args: &[String]) -> Result<Vec<String>> {
    let output = Command::new("rg")
        .args(args)
        .current_dir(cwd)
        .output()
        .context("failed to execute `rg` for Grep tool")?;
    let status = output.status.code().unwrap_or_default();
    if !output.status.success() && status != 1 {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("rg exited with status code {status}");
        }
        bail!("rg exited with status code {status}: {stderr}");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>())
}

fn run_grep_fallback(
    cwd: &Path,
    absolute_target: &Path,
    input: &ClaudeGrepInput,
    mode: GrepMode,
) -> Result<Vec<String>> {
    if input.multiline.unwrap_or(false) {
        bail!("multiline Grep requires `rg` to be installed");
    }

    let mut command = Command::new("grep");
    command
        .arg("-r")
        .arg("--binary-files=without-match")
        .arg("--devices=skip");
    if input.case_insensitive.unwrap_or(false) {
        command.arg("-i");
    }
    command.arg("-E");
    append_grep_mode_flags(&mut command, mode, input);
    append_grep_exclusions(&mut command);
    append_pseudo_fs_exclusions_grep(&mut command, absolute_target);
    append_grep_file_filters(
        &mut command,
        input.glob.as_deref(),
        input.file_type.as_deref(),
    );
    command
        .arg("--")
        .arg(&input.pattern)
        .arg(absolute_target)
        .current_dir(cwd);

    let output = command
        .output()
        .context("failed to execute `grep` fallback for Grep tool")?;
    let status = output.status.code().unwrap_or_default();
    if !output.status.success() && status != 1 {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("grep exited with status code {status}");
        }
        bail!("grep exited with status code {status}: {stderr}");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>())
}

fn append_grep_mode_flags(command: &mut Command, mode: GrepMode, input: &ClaudeGrepInput) {
    match mode {
        GrepMode::FilesWithMatches => {
            command.arg("-l");
        }
        GrepMode::Count => {
            command.arg("-c");
        }
        GrepMode::Content => {
            if input.show_line_numbers.unwrap_or(true) {
                command.arg("-n");
            }
            if let Some(value) = input.context.or(input.context_short) {
                command.arg("-C").arg(value.to_string());
                return;
            }
            if let Some(value) = input.before_context {
                command.arg("-B").arg(value.to_string());
            }
            if let Some(value) = input.after_context {
                command.arg("-A").arg(value.to_string());
            }
        }
    }
}

fn append_grep_exclusions(command: &mut Command) {
    for dir in VCS_DIRECTORIES_TO_EXCLUDE {
        command.arg(format!("--exclude-dir={dir}"));
    }
}

/// True when the grep target is the root `/`. Triggers
/// pseudo-FS exclusions in both the ripgrep and GNU-grep paths.
fn target_includes_pseudo_fs_roots(target: &Path) -> bool {
    target == Path::new("/")
}

/// ripgrep variant: pass `--glob '!proc'` etc. so the walker skips
/// `proc`/`sys`/`dev`/`run` basenames anywhere in the search tree.
/// Only fires when the target is `/`, so non-system grep targets
/// that incidentally contain a `proc/` subdir are unaffected.
fn append_pseudo_fs_exclusions_rg(args: &mut Vec<String>, target: &Path) {
    if !target_includes_pseudo_fs_roots(target) {
        return;
    }
    for dir in PSEUDO_FS_DIRS {
        args.push("--glob".to_string());
        args.push(format!("!{dir}"));
    }
}

/// GNU grep fallback variant: pass `--exclude-dir=proc` etc. Only
/// fires when the target is `/` for the same reason as the ripgrep
/// helper above.
fn append_pseudo_fs_exclusions_grep(command: &mut Command, target: &Path) {
    if !target_includes_pseudo_fs_roots(target) {
        return;
    }
    for dir in PSEUDO_FS_DIRS {
        command.arg(format!("--exclude-dir={dir}"));
    }
}

fn append_grep_file_filters(
    command: &mut Command,
    glob_value: Option<&str>,
    file_type: Option<&str>,
) {
    if let Some(file_type) = file_type.filter(|value| !value.trim().is_empty()) {
        for pattern in file_type_patterns(file_type) {
            command.arg(format!("--include={pattern}"));
        }
    }

    let Some(glob_value) = glob_value.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    for token in glob_value.split_whitespace() {
        for part in token
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            if let Some(excluded) = part.strip_prefix('!') {
                command.arg(format!("--exclude={excluded}"));
            } else {
                command.arg(format!("--include={part}"));
            }
        }
    }
}

fn file_type_patterns(file_type: &str) -> &'static [&'static str] {
    match file_type {
        "c" => &["*.c", "*.h"],
        "cc" | "cpp" | "cxx" => &["*.cc", "*.cpp", "*.cxx", "*.h", "*.hh", "*.hpp", "*.hxx"],
        "cs" => &["*.cs"],
        "css" => &["*.css", "*.scss", "*.sass", "*.less"],
        "go" => &["*.go"],
        "h" => &["*.h", "*.hh", "*.hpp", "*.hxx"],
        "html" => &["*.html", "*.htm"],
        "java" => &["*.java"],
        "js" => &["*.js", "*.cjs", "*.mjs"],
        "json" => &["*.json"],
        "jsx" => &["*.jsx"],
        "kt" | "kotlin" => &["*.kt", "*.kts"],
        "md" | "markdown" => &["*.md", "*.markdown"],
        "php" => &["*.php"],
        "proto" => &["*.proto"],
        "py" | "python" => &["*.py"],
        "rb" | "ruby" => &["*.rb"],
        "rs" | "rust" => &["*.rs"],
        "sh" | "shell" => &["*.sh", "*.bash", "*.zsh"],
        "sql" => &["*.sql"],
        "swift" => &["*.swift"],
        "toml" => &["*.toml"],
        "ts" => &["*.ts", "*.cts", "*.mts"],
        "tsx" => &["*.tsx"],
        "txt" | "text" => &["*.txt"],
        "xml" => &["*.xml"],
        "yaml" | "yml" => &["*.yaml", "*.yml"],
        _ => &[],
    }
}

fn rg_available() -> bool {
    Command::new("rg").arg("--version").output().is_ok()
}

fn sort_paths_by_mtime_desc(cwd: &Path, paths: Vec<String>) -> Vec<String> {
    let mut scored = paths
        .into_iter()
        .map(|path| {
            let mtime = file_mtime_ms(cwd, &path);
            (path, mtime)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    scored.into_iter().map(|(path, _)| path).collect()
}

fn file_mtime_ms(cwd: &Path, path_text: &str) -> u128 {
    let path = Path::new(path_text);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    fs::metadata(joined)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(system_time_to_epoch_ms)
        .unwrap_or(0)
}

fn system_time_to_epoch_ms(time: SystemTime) -> Option<u128> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_millis())
}

fn apply_head_limit(
    items: &[String],
    limit: Option<usize>,
    offset: usize,
) -> (Option<usize>, Vec<String>) {
    if limit == Some(0) {
        return (None, items.iter().skip(offset).cloned().collect::<Vec<_>>());
    }

    let effective_limit = limit.unwrap_or(DEFAULT_HEAD_LIMIT);
    let sliced = items
        .iter()
        .skip(offset)
        .take(effective_limit)
        .cloned()
        .collect::<Vec<_>>();
    let truncated = items.len().saturating_sub(offset) > effective_limit;
    (truncated.then_some(effective_limit), sliced)
}

fn files_mode_output(
    cwd: &Path,
    entries: Vec<String>,
    applied_limit: Option<usize>,
    offset: usize,
) -> Value {
    let filenames = entries
        .iter()
        .map(|entry| to_relative_path(cwd, entry))
        .collect::<Vec<_>>();

    let mut object = Map::new();
    object.insert(
        "mode".to_string(),
        Value::String("files_with_matches".to_string()),
    );
    object.insert("filenames".to_string(), json!(filenames));
    object.insert("numFiles".to_string(), json!(entries.len()));
    if let Some(applied_limit) = applied_limit {
        object.insert("appliedLimit".to_string(), json!(applied_limit));
    }
    if offset > 0 {
        object.insert("appliedOffset".to_string(), json!(offset));
    }
    Value::Object(object)
}

fn content_mode_output(
    cwd: &Path,
    entries: Vec<String>,
    applied_limit: Option<usize>,
    offset: usize,
) -> Value {
    let lines = entries
        .iter()
        .map(|entry| relativize_content_line(cwd, entry))
        .collect::<Vec<_>>();

    let mut object = Map::new();
    object.insert("mode".to_string(), Value::String("content".to_string()));
    object.insert("numFiles".to_string(), json!(0));
    object.insert("filenames".to_string(), json!(Vec::<String>::new()));
    object.insert("content".to_string(), Value::String(lines.join("\n")));
    object.insert("numLines".to_string(), json!(lines.len()));
    if let Some(applied_limit) = applied_limit {
        object.insert("appliedLimit".to_string(), json!(applied_limit));
    }
    if offset > 0 {
        object.insert("appliedOffset".to_string(), json!(offset));
    }
    Value::Object(object)
}

fn count_mode_output(
    cwd: &Path,
    entries: Vec<String>,
    applied_limit: Option<usize>,
    offset: usize,
) -> Value {
    let lines = entries
        .iter()
        .map(|entry| relativize_count_line(cwd, entry))
        .collect::<Vec<_>>();

    let mut total_matches = 0usize;
    let mut file_count = 0usize;
    for line in &lines {
        if let Some((_, count)) = line.rsplit_once(':') {
            if let Ok(parsed) = count.trim().parse::<usize>() {
                total_matches = total_matches.saturating_add(parsed);
                file_count = file_count.saturating_add(1);
            }
        }
    }

    let mut object = Map::new();
    object.insert("mode".to_string(), Value::String("count".to_string()));
    object.insert("numFiles".to_string(), json!(file_count));
    object.insert("filenames".to_string(), json!(Vec::<String>::new()));
    object.insert("content".to_string(), Value::String(lines.join("\n")));
    object.insert("numMatches".to_string(), json!(total_matches));
    if let Some(applied_limit) = applied_limit {
        object.insert("appliedLimit".to_string(), json!(applied_limit));
    }
    if offset > 0 {
        object.insert("appliedOffset".to_string(), json!(offset));
    }
    Value::Object(object)
}

fn relativize_content_line(cwd: &Path, line: &str) -> String {
    let Some((path, remainder)) = line.split_once(':') else {
        return line.to_string();
    };
    format!("{}:{remainder}", to_relative_path(cwd, path))
}

fn relativize_count_line(cwd: &Path, line: &str) -> String {
    let Some((path, count)) = line.rsplit_once(':') else {
        return line.to_string();
    };
    format!("{}:{count}", to_relative_path(cwd, path))
}

fn to_relative_path(cwd: &Path, path_text: &str) -> String {
    let path = Path::new(path_text);
    if path.is_absolute() {
        if let Ok(relative) = path.strip_prefix(cwd) {
            return relative.to_string_lossy().replace('\\', "/");
        }
    }
    path_text.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pseudo_fs_exclusions_only_fire_for_root_target() {
        assert!(target_includes_pseudo_fs_roots(Path::new("/")));
        // Anything more specific is the user's intent — leave alone.
        assert!(!target_includes_pseudo_fs_roots(Path::new("/proc")));
        assert!(!target_includes_pseudo_fs_roots(Path::new("/app")));
        assert!(!target_includes_pseudo_fs_roots(Path::new("/home/user")));
    }

    #[test]
    fn pseudo_fs_exclusions_rg_emits_glob_pairs_for_root_only() {
        let mut args: Vec<String> = Vec::new();
        append_pseudo_fs_exclusions_rg(&mut args, Path::new("/"));
        // For each pseudo-FS dir we push two args: --glob then !dir.
        assert_eq!(args.len(), PSEUDO_FS_DIRS.len() * 2);
        for chunk in args.chunks_exact(2) {
            assert_eq!(chunk[0], "--glob");
            assert!(chunk[1].starts_with('!'));
        }

        let mut args2: Vec<String> = Vec::new();
        append_pseudo_fs_exclusions_rg(&mut args2, Path::new("/app"));
        assert!(
            args2.is_empty(),
            "non-root targets should not get pseudo-FS exclusions"
        );
    }

    #[test]
    fn grep_files_with_matches_mode_returns_expected_shape() {
        if !rg_available() {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/a.rs"), "fn hello() {}\n").unwrap();
        fs::write(temp.path().join("src/b.rs"), "fn world() {}\n").unwrap();

        let output = execute_claude_grep(
            temp.path(),
            &[],
            false,
            json!({
                "pattern": "fn",
                "path": "src",
                "output_mode": "files_with_matches"
            }),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["mode"], "files_with_matches");
        assert_eq!(parsed["numFiles"], 2);
        assert!(parsed["filenames"].as_array().unwrap().len() >= 2);
    }

    #[test]
    fn grep_count_mode_reports_matches() {
        if !rg_available() {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("note.txt"), "abc\nabc\n").unwrap();

        let output = execute_claude_grep(
            temp.path(),
            &[],
            false,
            json!({
                "pattern": "abc",
                "output_mode": "count"
            }),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["mode"], "count");
        assert!(parsed["numMatches"].as_u64().unwrap() >= 2);
    }

    #[test]
    fn grep_rejects_working_directory_escape() {
        let temp = tempfile::tempdir().unwrap();
        let error = execute_claude_grep(
            temp.path(),
            &[],
            false,
            json!({
                "pattern": "abc",
                "path": "../"
            }),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("outside the current working directories"));
    }

    #[test]
    fn grep_searches_added_working_directories() {
        if !rg_available() {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("repo");
        let extra = temp.path().join("extra");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&extra).unwrap();
        fs::write(extra.join("note.txt"), "abc\nabc\n").unwrap();

        let output = execute_claude_grep(
            &cwd,
            &[extra.clone()],
            false,
            json!({
                "pattern": "abc",
                "path": extra.display().to_string(),
                "output_mode": "count"
            }),
        )
        .unwrap();

        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["mode"], "count");
        assert!(parsed["content"]
            .as_str()
            .is_some_and(|text| text.contains(&extra.join("note.txt").display().to_string())));
    }

    #[test]
    fn head_limit_defaults_to_claude_value() {
        let data = vec![
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];
        let (limit, sliced) = apply_head_limit(&data, Some(2), 1);
        assert_eq!(limit, Some(2));
        assert_eq!(sliced, vec!["2".to_string(), "3".to_string()]);
    }

    #[test]
    fn grep_fallback_supports_count_mode() {
        let temp = tempfile::tempdir().unwrap();
        let note = temp.path().join("note.txt");
        fs::write(&note, "abc\nabc\n").unwrap();

        let output = run_grep_fallback(
            temp.path(),
            temp.path(),
            &ClaudeGrepInput {
                pattern: "abc".to_string(),
                path: None,
                glob: None,
                output_mode: Some("count".to_string()),
                before_context: None,
                after_context: None,
                context_short: None,
                context: None,
                show_line_numbers: None,
                case_insensitive: None,
                file_type: None,
                head_limit: None,
                offset: None,
                multiline: None,
            },
            GrepMode::Count,
        )
        .unwrap();

        assert_eq!(output, vec![format!("{}:2", note.display())]);
    }
}
