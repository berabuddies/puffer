//! One-replay execution: spawn sandbox, run puffer, capture artifact.

use crate::pr_corpus::CorpusEntry;
use crate::sandbox::Sandbox;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::time::Duration;

/// A replay arm: which skill (if any) was loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Arm {
    /// No skill loaded (baseline).
    NoSkill,
    /// Direct-prompt-generated skill loaded.
    Direct,
    /// /genskill GEPA-generated skill loaded.
    Gepa,
}

impl Arm {
    /// Parses a CLI string into an Arm.
    pub fn parse(s: &str) -> Result<Arm> {
        match s {
            "no-skill" => Ok(Arm::NoSkill),
            "direct" => Ok(Arm::Direct),
            "gepa" => Ok(Arm::Gepa),
            _ => Err(anyhow!(
                "unknown arm {s}; expected no-skill | direct | gepa"
            )),
        }
    }

    /// Returns the kebab-case CLI spelling for this arm.
    pub fn as_str(self) -> &'static str {
        match self {
            Arm::NoSkill => "no-skill",
            Arm::Direct => "direct",
            Arm::Gepa => "gepa",
        }
    }
}

/// How a replay terminated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Target test passed.
    Pass,
    /// Test still failed after agent's claim of completion.
    WrongFix,
    /// Agent gave up.
    GaveUp,
    /// Wall-clock budget exceeded.
    WallTimeout,
    /// Tool-call cap exceeded.
    ToolBudget,
    /// Token budget exceeded.
    TokenBudget,
}

/// Token usage breakdown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub tool_results: u64,
    pub total: u64,
}

/// One tool call recorded during the replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub input: serde_json::Value,
    pub output_size: u64,
    pub ts: String,
}

/// Outcome of running the target test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestOutcome {
    pub command: String,
    pub exit_code: i32,
    pub stdout_tail: String,
}

/// Full replay artifact stored at reports/<run_date>/<pr>-<arm>.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayArtifact {
    pub pr: String,
    pub arm: Arm,
    pub outcome: Outcome,
    pub wall_seconds: u64,
    pub tool_calls: u64,
    pub tokens: Tokens,
    pub tool_call_log: Vec<ToolCall>,
    pub final_diff: String,
    pub test_outcome: Option<TestOutcome>,
}

/// Configuration for one replay.
pub struct ReplayConfig<'a> {
    pub corpus_entry: &'a CorpusEntry,
    pub arm: Arm,
    pub puffer_bin_host_path: PathBuf,
    pub agent_provider: String,
    pub agent_model: String,
    pub agent_effort: Option<String>,
    pub image: String,
    pub wall_budget: Duration,
    pub tool_budget: u64,
    pub token_budget: u64,
    pub run_date_dir: PathBuf,
}

/// Runs one replay end-to-end. Writes the artifact JSON and returns it.
pub async fn run_one(cfg: ReplayConfig<'_>) -> Result<ReplayArtifact> {
    let started = std::time::Instant::now();
    let test_files_dir = cfg.corpus_entry.dir.join("tests");
    let sandbox = Sandbox::start(
        &cfg.image,
        &cfg.puffer_bin_host_path,
        &cfg.corpus_entry.meta.base_commit,
        &test_files_dir,
    )
    .await
    .context("starting sandbox")?;

    let test_filters = test_filters_for(cfg.corpus_entry);
    let pre_check = run_target_test(&sandbox, &test_filters).await.ok();
    tracing::info!(?pre_check, "pre-replay test status");

    let mut puffer_args = vec![
        "non-interactive".to_string(),
        "--provider".to_string(),
        cfg.agent_provider.clone(),
        "--model".to_string(),
        cfg.agent_model.clone(),
        "--user-message".to_string(),
        cfg.corpus_entry.meta.task_prompt.clone(),
        "--max-tool-calls".to_string(),
        cfg.tool_budget.to_string(),
        "--max-tokens".to_string(),
        cfg.token_budget.to_string(),
        "--emit-artifact".to_string(),
        "/tmp/replay-artifact.json".to_string(),
        "--artifact-pr".to_string(),
        cfg.corpus_entry.id.clone(),
        "--artifact-arm".to_string(),
        cfg.arm.as_str().to_string(),
    ];
    if let Some(effort) = cfg.agent_effort.as_deref() {
        puffer_args.push("--effort".to_string());
        puffer_args.push(effort.to_string());
    }
    if let Some(skill_path) = skill_path_for(&cfg) {
        puffer_args.push("--load-skill".to_string());
        puffer_args.push(skill_path);
    }

    let exec_args: Vec<&str> = std::iter::once("puffer")
        .chain(puffer_args.iter().map(String::as_str))
        .collect();

    let exec_result = tokio::time::timeout(cfg.wall_budget, sandbox.exec(&exec_args)).await;
    let outcome_kind = match exec_result {
        Err(_) => Outcome::WallTimeout,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "puffer exec failed");
            Outcome::GaveUp
        }
        Ok(Ok(_)) => Outcome::Pass,
    };

    let pulled = sandbox
        .exec(&["bash", "-c", "cat /tmp/replay-artifact.json"])
        .await;
    let mut artifact = if let Ok((stdout, _)) = pulled {
        serde_json::from_str::<ReplayArtifact>(&stdout).unwrap_or_else(|_| empty_artifact(&cfg))
    } else {
        empty_artifact(&cfg)
    };

    if matches!(outcome_kind, Outcome::WallTimeout) {
        artifact.outcome = Outcome::WallTimeout;
    }

    let test_run = run_target_test(&sandbox, &test_filters).await;
    if let Ok(test_outcome) = test_run {
        let exit_code = test_outcome.exit_code;
        artifact.test_outcome = Some(test_outcome);
        if exit_code == 0 && !matches!(artifact.outcome, Outcome::WallTimeout) {
            artifact.outcome = Outcome::Pass;
        } else if exit_code != 0 && matches!(artifact.outcome, Outcome::Pass) {
            artifact.outcome = Outcome::WrongFix;
        }
    }

    artifact.wall_seconds = started.elapsed().as_secs();

    std::fs::create_dir_all(&cfg.run_date_dir)
        .with_context(|| format!("creating {}", cfg.run_date_dir.display()))?;
    let artifact_path = cfg
        .run_date_dir
        .join(format!("{}-{:?}.json", cfg.corpus_entry.id, cfg.arm));
    std::fs::write(&artifact_path, serde_json::to_string_pretty(&artifact)?)?;
    tracing::info!(path = %artifact_path.display(), "wrote replay artifact");

    Ok(artifact)
}

fn empty_artifact(cfg: &ReplayConfig<'_>) -> ReplayArtifact {
    ReplayArtifact {
        pr: cfg.corpus_entry.id.clone(),
        arm: cfg.arm,
        outcome: Outcome::GaveUp,
        wall_seconds: 0,
        tool_calls: 0,
        tokens: Tokens::default(),
        tool_call_log: Vec::new(),
        final_diff: String::new(),
        test_outcome: None,
    }
}

fn skill_path_for(cfg: &ReplayConfig<'_>) -> Option<String> {
    match cfg.arm {
        Arm::NoSkill => None,
        Arm::Direct => Some(format!(
            "/host/{}/skills/direct/SKILL.md",
            cfg.corpus_entry.dir.display()
        )),
        Arm::Gepa => Some(format!(
            "/host/{}/skills/gepa/SKILL.md",
            cfg.corpus_entry.dir.display()
        )),
    }
}

fn test_filters_for(entry: &CorpusEntry) -> Vec<String> {
    if let Ok(filters) = explicit_test_filters(&entry.dir.join("test_filters.txt")) {
        if !filters.is_empty() {
            return filters;
        }
    }

    let mut filters = corpus_test_files(&entry.dir.join("tests"))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|path| corpus_test_filter(&path))
        .collect::<Vec<_>>();
    filters.sort();
    filters.dedup();
    if filters.is_empty() {
        vec![entry.id.clone()]
    } else {
        filters
    }
}

fn explicit_test_filters(path: &Path) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let mut filters = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    filters.sort();
    filters.dedup();
    Ok(filters)
}

fn corpus_test_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_corpus_test_files(root, root, &mut files)?;
    Ok(files)
}

fn collect_corpus_test_files(root: &Path, current: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for dent in
        std::fs::read_dir(current).with_context(|| format!("reading {}", current.display()))?
    {
        let dent = dent?;
        let path = dent.path();
        if path.is_dir() {
            collect_corpus_test_files(root, &path, files)?;
        } else if path.is_file() {
            files.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn corpus_test_filter(path: &Path) -> Option<String> {
    let path = path.to_string_lossy().replace('\\', "/");
    if path.ends_with("/CMakeLists.txt") {
        return None;
    }

    if path.starts_with("Libraries/LibJS/Tests/") && path.ends_with(".js") {
        return Some(path);
    }

    if path.starts_with("Libraries/LibWasm/Tests/Executor/") && path.ends_with(".js") {
        return Some(path);
    }

    if !path.starts_with("Tests/") {
        return None;
    }

    if let Some(stem) = path
        .strip_prefix("Tests/LibWeb/Text/expected/")
        .or_else(|| path.strip_prefix("Tests/LibWeb/Layout/expected/"))
        .and_then(|value| value.strip_suffix(".txt"))
    {
        let prefix = if path.starts_with("Tests/LibWeb/Text/expected/") {
            "Tests/LibWeb/Text/input/"
        } else {
            "Tests/LibWeb/Layout/input/"
        };
        return Some(format!("{prefix}{stem}.html"));
    }

    if path.ends_with(".html") || path.ends_with(".cpp") || path.ends_with(".js") {
        return Some(path);
    }
    None
}

async fn run_target_test(sandbox: &Sandbox, filters: &[String]) -> Result<TestOutcome> {
    let command = test_command_for(filters);
    let out = sandbox.exec_status(&["bash", "-lc", &command]).await?;
    let combined = if out.stderr.trim().is_empty() {
        out.stdout
    } else {
        format!("{}\n{}", out.stdout, out.stderr)
    };
    Ok(TestOutcome {
        command,
        exit_code: out.exit_code,
        stdout_tail: tail(&combined, 4_000),
    })
}

fn test_command_for(filters: &[String]) -> String {
    let web_filters = filters
        .iter()
        .filter_map(|filter| libweb_filter_arg(filter))
        .collect::<Vec<_>>();
    let ctest_patterns = filters
        .iter()
        .filter_map(|filter| ctest_pattern_arg(filter))
        .collect::<Vec<_>>();
    let js_filters = filters
        .iter()
        .filter_map(|filter| js_test_filter_arg(filter))
        .collect::<Vec<_>>();
    let wasm_filters = filters
        .iter()
        .filter_map(|filter| wasm_test_filter_arg(filter))
        .collect::<Vec<_>>();
    let unsupported_filters = filters
        .iter()
        .filter(|filter| {
            libweb_filter_arg(filter).is_none()
                && ctest_pattern_arg(filter).is_none()
                && js_test_filter_arg(filter).is_none()
                && wasm_test_filter_arg(filter).is_none()
        })
        .collect::<Vec<_>>();

    let mut commands = vec!["status=0".to_string()];
    if !web_filters.is_empty() {
        let web_args = web_filters
            .iter()
            .map(|filter| format!("-f {}", shell_quote(filter)))
            .collect::<Vec<_>>()
            .join(" ");
        commands.push(format!(
            "echo '=== target web tests ==='; \
             if [ -f Meta/ladybird.py ]; then \
                 python3 Meta/ladybird.py run --jobs 1 test-web {web_args}; \
             elif [ -x Meta/ladybird.sh ]; then \
                 ./Meta/ladybird.sh run test-web {web_args}; \
             elif command -v test-web >/dev/null 2>&1; then \
                 test-web {web_args}; \
             else \
                 echo 'no Ladybird web test runner found' >&2; exit 127; \
             fi; \
             rc=$?; \
             if [ $rc -ne 0 ]; then status=$rc; fi"
        ));
    }

    for (target, pattern, workdir) in ctest_patterns {
        let quoted_target = shell_quote(&target);
        let quoted_pattern = shell_quote(&pattern);
        let case_command = workdir
            .as_deref()
            .map(|dir| {
                format!(
                    "(cd {} && /work/ladybird/Build/release/bin/{target} {quoted_pattern})",
                    shell_quote(dir)
                )
            })
            .unwrap_or_else(|| {
                format!("/work/ladybird/Build/release/bin/{target} {quoted_pattern}")
            });
        commands.push(format!(
            "echo '=== target unit test: {target} {pattern} ==='; \
             if [ -f Meta/ladybird.py ]; then \
                 python3 Meta/ladybird.py build --jobs 1 {quoted_target}; \
                 rc=$?; \
                 if [ $rc -eq 0 ]; then {case_command}; rc=$?; fi; \
             elif [ -x Meta/ladybird.sh ]; then \
                 ./Meta/ladybird.sh run {quoted_target} {quoted_pattern}; \
                 rc=$?; \
             elif command -v ctest >/dev/null 2>&1; then \
                 ctest --output-on-failure -R {quoted_target}; \
                 rc=$?; \
             else \
                 echo 'no Ladybird unit-test runner found' >&2; exit 127; \
             fi; \
             if [ $rc -ne 0 ]; then status=$rc; fi"
        ));
    }

    if !js_filters.is_empty() {
        commands.push(js_runner_command(
            "test-js",
            "Libraries/LibJS/Tests",
            &js_filters,
        ));
    }

    if !wasm_filters.is_empty() {
        commands.push(js_runner_command(
            "test-wasm",
            "Libraries/LibWasm/Tests",
            &wasm_filters,
        ));
    }

    for filter in unsupported_filters {
        let quoted_filter = shell_quote(filter);
        commands.push(format!(
            "echo unsupported target test filter: {quoted_filter} >&2; status=127"
        ));
    }

    commands.push("exit $status".to_string());
    commands.join("; ")
}

fn libweb_filter_arg(filter: &str) -> Option<String> {
    filter
        .strip_prefix("Tests/LibWeb/")
        .map(ToString::to_string)
}

fn js_test_filter_arg(filter: &str) -> Option<String> {
    filter
        .strip_prefix("Libraries/LibJS/Tests/")
        .filter(|path| path.ends_with(".js"))
        .map(ToString::to_string)
}

fn wasm_test_filter_arg(filter: &str) -> Option<String> {
    filter
        .strip_prefix("Libraries/LibWasm/Tests/")
        .filter(|path| path.starts_with("Executor/") && path.ends_with(".js"))
        .map(ToString::to_string)
}

fn js_runner_command(target: &str, root: &str, filters: &[String]) -> String {
    let quoted_target = shell_quote(target);
    let filter_args = filters
        .iter()
        .map(|filter| format!("-f {}", shell_quote(filter)))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "echo '=== target js tests: {target} ==='; \
         if [ -f Meta/ladybird.py ]; then \
             python3 Meta/ladybird.py build --jobs 1 {quoted_target}; \
             rc=$?; \
             if [ $rc -eq 0 ]; then \
                 /work/ladybird/Build/release/bin/{target} {filter_args} {root} Libraries/LibJS/Tests/test-common.js; \
                 rc=$?; \
             fi; \
         elif [ -x Meta/ladybird.sh ]; then \
             ./Meta/ladybird.sh run {quoted_target} {filter_args}; \
             rc=$?; \
         else \
             echo 'no Ladybird JS test runner found' >&2; exit 127; \
         fi; \
         if [ $rc -ne 0 ]; then status=$rc; fi"
    )
}

fn ctest_pattern_arg(filter: &str) -> Option<(String, String, Option<String>)> {
    let (path, case) = filter
        .split_once(':')
        .map_or((filter, "*"), |(path, case)| (path, case));
    let stem = Path::new(path).file_stem()?.to_str()?;
    let workdir = Path::new(path)
        .parent()
        .and_then(Path::to_str)
        .map(ToString::to_string);
    path.ends_with(".cpp")
        .then(|| (stem.to_string(), case.to_string(), workdir))
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn tail(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        s[s.len() - n..].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_parses() {
        assert_eq!(Arm::parse("no-skill").unwrap(), Arm::NoSkill);
        assert_eq!(Arm::parse("direct").unwrap(), Arm::Direct);
        assert_eq!(Arm::parse("gepa").unwrap(), Arm::Gepa);
        assert!(Arm::parse("garbage").is_err());
    }

    #[test]
    fn shell_quote_wraps_simple_values() {
        assert_eq!(
            shell_quote("Tests/LibWeb/foo.html"),
            "'Tests/LibWeb/foo.html'"
        );
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(
            shell_quote("LibRegex nested 'or'"),
            "'LibRegex nested '\"'\"'or'\"'\"''"
        );
    }

    #[test]
    fn test_command_uses_web_runner_for_libweb_filters() {
        let filters = vec![
            "Tests/LibWeb/Text/input/wpt-import/css/foo/bar.html".to_string(),
            "Tests/LibWeb/Crash/CSS/foo.html".to_string(),
        ];
        let command = test_command_for(&filters);
        assert!(command.contains("python3 Meta/ladybird.py run --jobs 1 test-web"));
        assert!(command.contains("-f 'Text/input/wpt-import/css/foo/bar.html'"));
        assert!(command.contains("-f 'Crash/CSS/foo.html'"));
        assert!(!command.contains("No tests were found"));
    }

    #[test]
    fn test_command_uses_ctest_patterns_for_cpp_tests() {
        let filters = vec![
            "Tests/LibRegex/TestRegex.cpp".to_string(),
            "Tests/LibURL/TestPublicSuffix.cpp".to_string(),
        ];
        let command = test_command_for(&filters);
        assert!(command.contains("python3 Meta/ladybird.py build --jobs 1 'TestRegex'"));
        assert!(command.contains("/work/ladybird/Build/release/bin/TestRegex '*'"));
        assert!(command.contains("python3 Meta/ladybird.py build --jobs 1 'TestPublicSuffix'"));
        assert!(command.contains("/work/ladybird/Build/release/bin/TestPublicSuffix '*'"));
    }

    #[test]
    fn test_command_passes_cpp_case_pattern_to_unit_binary() {
        let filters = vec!["Tests/LibGfx/TestImageDecoder.cpp:test_gif_empty_lzw_data".to_string()];
        let command = test_command_for(&filters);
        assert!(command.contains("python3 Meta/ladybird.py build --jobs 1 'TestImageDecoder'"));
        assert!(command.contains(
            "(cd 'Tests/LibGfx' && /work/ladybird/Build/release/bin/TestImageDecoder 'test_gif_empty_lzw_data')"
        ));
        assert!(command.contains("ctest --output-on-failure -R 'TestImageDecoder'"));
    }

    #[test]
    fn test_command_isolates_cpp_case_workdirs() {
        let filters = vec![
            "Tests/LibGfx/TestImageDecoder.cpp:a".to_string(),
            "Tests/LibGfx/TestImageDecoder.cpp:b".to_string(),
        ];
        let command = test_command_for(&filters);
        assert_eq!(command.matches("(cd 'Tests/LibGfx' &&").count(), 2);
        assert!(command.contains("=== target unit test: TestImageDecoder a ==="));
        assert!(command.contains("=== target unit test: TestImageDecoder b ==="));
    }

    #[test]
    fn test_command_rejects_unsupported_filters() {
        let filters = vec!["Libraries/LibWasm/Tests/Fixtures/Modules/foo.wasm".to_string()];
        let command = test_command_for(&filters);
        assert!(command.contains(
            "unsupported target test filter: 'Libraries/LibWasm/Tests/Fixtures/Modules/foo.wasm'"
        ));
        assert!(command.contains("status=127"));
    }

    #[test]
    fn test_command_runs_libjs_filters_with_test_js() {
        let filters = vec!["Libraries/LibJS/Tests/regress/inline-caching.js".to_string()];
        let command = test_command_for(&filters);
        assert!(command.contains("python3 Meta/ladybird.py build --jobs 1 'test-js'"));
        assert!(command.contains(
            "/work/ladybird/Build/release/bin/test-js -f 'regress/inline-caching.js' Libraries/LibJS/Tests Libraries/LibJS/Tests/test-common.js"
        ));
    }

    #[test]
    fn test_command_runs_libwasm_executor_filters_with_test_wasm() {
        let filters =
            vec!["Libraries/LibWasm/Tests/Executor/test-memory_fill-order.js".to_string()];
        let command = test_command_for(&filters);
        assert!(command.contains("python3 Meta/ladybird.py build --jobs 1 'test-wasm'"));
        assert!(command.contains(
            "/work/ladybird/Build/release/bin/test-wasm -f 'Executor/test-memory_fill-order.js' Libraries/LibWasm/Tests Libraries/LibJS/Tests/test-common.js"
        ));
    }

    #[test]
    fn corpus_test_filter_maps_text_expected_to_input() {
        assert_eq!(
            corpus_test_filter(Path::new(
                "Tests/LibWeb/Text/expected/wpt-import/css/foo/bar.txt"
            )),
            Some("Tests/LibWeb/Text/input/wpt-import/css/foo/bar.html".to_string())
        );
    }

    #[test]
    fn corpus_test_filter_maps_layout_expected_to_input() {
        assert_eq!(
            corpus_test_filter(Path::new("Tests/LibWeb/Layout/expected/flex/foo.txt")),
            Some("Tests/LibWeb/Layout/input/flex/foo.html".to_string())
        );
    }

    #[test]
    fn corpus_test_filter_keeps_crash_and_cpp_tests() {
        assert_eq!(
            corpus_test_filter(Path::new("Tests/LibWeb/Crash/CSS/foo.html")),
            Some("Tests/LibWeb/Crash/CSS/foo.html".to_string())
        );
        assert_eq!(
            corpus_test_filter(Path::new("Tests/LibRegex/TestRegex.cpp")),
            Some("Tests/LibRegex/TestRegex.cpp".to_string())
        );
    }

    #[test]
    fn corpus_test_filter_ignores_build_metadata() {
        assert_eq!(
            corpus_test_filter(Path::new("Tests/LibURL/CMakeLists.txt")),
            None
        );
    }

    #[test]
    fn corpus_test_filter_keeps_libjs_and_libwasm_js_tests() {
        assert_eq!(
            corpus_test_filter(Path::new("Libraries/LibJS/Tests/regress/foo.js")),
            Some("Libraries/LibJS/Tests/regress/foo.js".to_string())
        );
        assert_eq!(
            corpus_test_filter(Path::new("Libraries/LibWasm/Tests/Executor/foo.js")),
            Some("Libraries/LibWasm/Tests/Executor/foo.js".to_string())
        );
        assert_eq!(
            corpus_test_filter(Path::new("Libraries/LibWasm/Tests/Fixtures/foo.wasm")),
            None
        );
    }

    #[test]
    fn explicit_test_filters_skip_comments_and_sort() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            "\n# comment\nTests/LibGfx/TestImageDecoder.cpp:b\nTests/LibGfx/TestImageDecoder.cpp:a\n",
        )
        .unwrap();
        assert_eq!(
            explicit_test_filters(tmp.path()).unwrap(),
            vec![
                "Tests/LibGfx/TestImageDecoder.cpp:a".to_string(),
                "Tests/LibGfx/TestImageDecoder.cpp:b".to_string()
            ]
        );
    }

    #[test]
    fn tail_short_unchanged() {
        assert_eq!(tail("abc", 10), "abc");
    }

    #[test]
    fn tail_truncates() {
        assert_eq!(tail(&"x".repeat(50), 10).len(), 10);
    }
}
