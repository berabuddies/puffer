use base64::{engine::general_purpose::STANDARD, Engine as _};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[test]
#[ignore = "launches a real daemon and Chrome to exercise the puffer browser CLI pipeline"]
fn browser_cli_pipeline_round_trips_snapshot_fill_click_eval() {
    let Some(chrome) = resolve_chrome_executable() else {
        eprintln!("skipping browser CLI e2e test because no Chrome executable was found");
        return;
    };

    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let _daemon = start_daemon(&workspace, &puffer_home, &chrome);

    let list = run_browser_json(&workspace, &puffer_home, &["list"]);
    let session_id = list
        .get("sessionId")
        .and_then(Value::as_str)
        .expect("list response should include a session id")
        .to_string();
    assert_eq!(list.get("action").and_then(Value::as_str), Some("list"));
    assert_eq!(
        list.pointer("/result/tabs")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let open = run_browser_json(&workspace, &puffer_home, &["open", &browser_fixture_url()]);
    assert_eq!(open.get("action").and_then(Value::as_str), Some("open"));
    assert_eq!(
        open.get("sessionId").and_then(Value::as_str),
        Some(session_id.as_str())
    );
    assert_eq!(
        open.pointer("/result/tabId").and_then(Value::as_str),
        Some("t1")
    );

    wait_for_eval_string(
        &workspace,
        &puffer_home,
        "document.title",
        "CLI Pipeline",
        Duration::from_secs(5),
    );

    let snapshot = run_browser_json(&workspace, &puffer_home, &["snapshot"]);
    assert_eq!(
        snapshot.get("sessionId").and_then(Value::as_str),
        Some(session_id.as_str())
    );
    assert_eq!(
        snapshot.pointer("/result/title").and_then(Value::as_str),
        Some("CLI Pipeline")
    );
    let refs = snapshot
        .pointer("/result/refs")
        .expect("snapshot response should include refs");
    let textarea_ref = find_ref(refs, "textbox", "Your name");
    let button_ref = find_ref(refs, "button", "Save");

    let fill = run_browser_json(
        &workspace,
        &puffer_home,
        &["fill", &textarea_ref, "Grace Hopper"],
    );
    assert_eq!(fill.get("action").and_then(Value::as_str), Some("fill"));
    assert_eq!(
        fill.pointer("/result/ok").and_then(Value::as_bool),
        Some(true)
    );

    let click = run_browser_json(&workspace, &puffer_home, &["click", &button_ref]);
    assert_eq!(click.get("action").and_then(Value::as_str), Some("click"));
    assert_eq!(
        click.pointer("/result/ok").and_then(Value::as_bool),
        Some(true)
    );

    wait_for_eval_string(
        &workspace,
        &puffer_home,
        "document.getElementById('result').textContent",
        "Grace Hopper",
        Duration::from_secs(5),
    );

    let quit = run_browser_json(&workspace, &puffer_home, &["quit"]);
    assert_eq!(quit.get("action").and_then(Value::as_str), Some("quit"));
    assert_eq!(
        quit.get("sessionId").and_then(Value::as_str),
        Some(session_id.as_str())
    );
    assert_eq!(
        quit.pointer("/result/tabs")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
}

#[test]
#[ignore = "launches a real daemon and Chrome to exercise browser upload end to end"]
fn browser_cli_upload_supports_direct_inputs_and_labels() {
    let Some(chrome) = resolve_chrome_executable() else {
        eprintln!("skipping browser CLI e2e test because no Chrome executable was found");
        return;
    };

    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let _daemon = start_daemon(&workspace, &puffer_home, &chrome);

    fs::write(workspace.join("direct-one.txt"), "direct-one").expect("write direct file one");
    fs::write(workspace.join("direct-two.txt"), "direct-two").expect("write direct file two");
    fs::write(workspace.join("label-one.txt"), "label-one").expect("write label file one");
    fs::write(workspace.join("label-two.txt"), "label-two").expect("write label file two");

    let open = run_browser_json(
        &workspace,
        &puffer_home,
        &["open", &browser_upload_fixture_url()],
    );
    assert_eq!(open.get("action").and_then(Value::as_str), Some("open"));

    wait_for_eval_string(
        &workspace,
        &puffer_home,
        "document.title",
        "CLI Upload",
        Duration::from_secs(5),
    );

    let snapshot = run_browser_json(&workspace, &puffer_home, &["snapshot"]);
    let refs = snapshot
        .pointer("/result/refs")
        .expect("snapshot response should include refs");
    let direct_ref = find_ref(refs, "file", "Direct upload");
    let label_ref = find_ref(refs, "label", "Upload by label");

    let direct = run_browser_json(
        &workspace,
        &puffer_home,
        &["upload", &direct_ref, "direct-one.txt", "direct-two.txt"],
    );
    assert_eq!(direct.get("action").and_then(Value::as_str), Some("upload"));
    assert_eq!(
        direct.pointer("/result/ok").and_then(Value::as_bool),
        Some(true)
    );

    wait_for_eval_string(
        &workspace,
        &puffer_home,
        "document.getElementById('direct-names').textContent",
        "direct-one.txt,direct-two.txt",
        Duration::from_secs(5),
    );
    wait_for_eval_string(
        &workspace,
        &puffer_home,
        "document.getElementById('direct-counts').textContent",
        "1/1",
        Duration::from_secs(5),
    );

    let label = run_browser_json(
        &workspace,
        &puffer_home,
        &["upload", &label_ref, "label-one.txt", "label-two.txt"],
    );
    assert_eq!(label.get("action").and_then(Value::as_str), Some("upload"));
    assert_eq!(
        label.pointer("/result/ok").and_then(Value::as_bool),
        Some(true)
    );

    wait_for_eval_string(
        &workspace,
        &puffer_home,
        "document.getElementById('label-names').textContent",
        "label-one.txt,label-two.txt",
        Duration::from_secs(5),
    );
    wait_for_eval_string(
        &workspace,
        &puffer_home,
        "document.getElementById('label-counts').textContent",
        "1/1",
        Duration::from_secs(5),
    );
}

#[test]
#[ignore = "launches a real daemon and Chrome to exercise the puffer browser screenshot pipeline"]
fn browser_cli_screenshot_writes_default_and_annotated_files() {
    let Some(chrome) = resolve_chrome_executable() else {
        eprintln!("skipping browser CLI e2e test because no Chrome executable was found");
        return;
    };

    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let _daemon = start_daemon(&workspace, &puffer_home, &chrome);

    let open = run_browser_json(&workspace, &puffer_home, &["open", &browser_fixture_url()]);
    assert_eq!(open.get("action").and_then(Value::as_str), Some("open"));
    wait_for_eval_string(
        &workspace,
        &puffer_home,
        "document.title",
        "CLI Pipeline",
        Duration::from_secs(5),
    );

    let screenshot = run_browser_json(&workspace, &puffer_home, &["screenshot"]);
    let screenshot_path = PathBuf::from(
        screenshot
            .pointer("/result/path")
            .and_then(Value::as_str)
            .expect("screenshot response should include a path"),
    );
    assert!(screenshot_path.starts_with(
        ConfigPaths::discover(&workspace)
            .workspace_config_dir
            .join("screenshots")
    ));
    assert_eq!(
        screenshot.pointer("/result/format").and_then(Value::as_str),
        Some("png")
    );
    assert_eq!(
        screenshot
            .pointer("/result/annotated")
            .and_then(Value::as_bool),
        Some(false)
    );
    assert!(screenshot.pointer("/result/refs").is_none());
    assert_file_prefix(&screenshot_path, b"\x89PNG\r\n\x1a\n");

    let annotated_relative = "captures/annotated.jpeg";
    let annotated = run_browser_json(
        &workspace,
        &puffer_home,
        &[
            "screenshot",
            annotated_relative,
            "--annotate",
            "--screenshot-format",
            "jpeg",
            "--screenshot-quality",
            "82",
        ],
    );
    let annotated_path = PathBuf::from(
        annotated
            .pointer("/result/path")
            .and_then(Value::as_str)
            .expect("annotated screenshot response should include a path"),
    );
    assert_eq!(annotated_path, workspace.join(annotated_relative));
    assert_eq!(
        annotated.pointer("/result/format").and_then(Value::as_str),
        Some("jpeg")
    );
    assert_eq!(
        annotated
            .pointer("/result/annotated")
            .and_then(Value::as_bool),
        Some(true)
    );
    let refs = annotated
        .pointer("/result/refs")
        .expect("annotated screenshot should include fresh refs");
    assert!(find_ref(refs, "textbox", "Your name").starts_with("@e"));
    assert!(find_ref(refs, "button", "Save").starts_with("@e"));
    assert!(annotated
        .pointer("/result/instruction")
        .and_then(Value::as_str)
        .is_some_and(|value| value.contains("annotated screenshot")));
    assert_file_prefix(&annotated_path, &[0xff, 0xd8, 0xff]);
}

struct DaemonHandle {
    child: Child,
    workspace: PathBuf,
    puffer_home: PathBuf,
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = run_browser_output(&self.workspace, &self.puffer_home, &["quit"]);
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn browser_fixture_url() -> String {
    let html = r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <title>CLI Pipeline</title>
  </head>
  <body>
    <main>
      <label for="name">Name</label>
      <textarea id="name" aria-label="Your name"></textarea>
      <button id="save" onclick="document.getElementById('result').textContent = document.getElementById('name').value;">Save</button>
      <output id="result">pending</output>
    </main>
  </body>
</html>"#;
    format!("data:text/html;base64,{}", STANDARD.encode(html))
}

fn browser_upload_fixture_url() -> String {
    let html = r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <title>CLI Upload</title>
  </head>
  <body>
    <main>
      <section>
        <label for="direct">Direct upload</label>
        <input id="direct" type="file" multiple aria-label="Direct upload">
        <output id="direct-names">pending</output>
        <output id="direct-counts">0/0</output>
      </section>
      <section>
        <label for="label-upload">Upload by label</label>
        <input id="label-upload" type="file" multiple style="position:absolute;left:-9999px;">
        <output id="label-names">pending</output>
        <output id="label-counts">0/0</output>
      </section>
    </main>
    <script>
      const bindUpload = (id, namesId, countsId) => {
        const input = document.getElementById(id);
        const names = document.getElementById(namesId);
        const counts = document.getElementById(countsId);
        const render = () => {
          names.textContent = Array.from(input.files || []).map((file) => file.name).join(",");
          counts.textContent = `${input.dataset.inputCount || "0"}/${input.dataset.changeCount || "0"}`;
        };
        input.addEventListener("input", () => {
          input.dataset.inputCount = String((Number(input.dataset.inputCount || "0") + 1));
          render();
        });
        input.addEventListener("change", () => {
          input.dataset.changeCount = String((Number(input.dataset.changeCount || "0") + 1));
          render();
        });
        render();
      };
      bindUpload("direct", "direct-names", "direct-counts");
      bindUpload("label-upload", "label-names", "label-counts");
    </script>
  </body>
</html>"#;
    format!("data:text/html;base64,{}", STANDARD.encode(html))
}

fn configured_workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let workspace = tempdir.path().join("workspace");
    let puffer_home = tempdir.path().join("puffer-home");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(&puffer_home).expect("puffer-home");
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).expect("dirs");
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate parent")
        .parent()
        .expect("repo root");
    std::os::unix::fs::symlink(repo_root.join("resources"), workspace.join("resources"))
        .expect("resource symlink");
    (tempdir, workspace, puffer_home)
}

fn find_ref(refs: &Value, role: &str, name: &str) -> String {
    refs.as_object()
        .expect("snapshot refs should be an object")
        .iter()
        .find_map(|(ref_id, entry)| {
            (entry.get("role").and_then(Value::as_str) == Some(role)
                && entry.get("name").and_then(Value::as_str) == Some(name))
            .then(|| ref_id.to_string())
        })
        .unwrap_or_else(|| {
            panic!(
                "missing ref for role={role:?} name={name:?} in refs:\n{}",
                serde_json::to_string_pretty(refs).expect("serialize refs")
            )
        })
}

fn assert_file_prefix(path: &Path, prefix: &[u8]) {
    let bytes = fs::read(path)
        .unwrap_or_else(|error| panic!("failed to read screenshot {}: {error}", path.display()));
    assert!(
        bytes.starts_with(prefix),
        "unexpected file prefix for {}: {:?}",
        path.display(),
        &bytes[..bytes.len().min(prefix.len() + 4)]
    );
}

fn resolve_chrome_executable() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PUFFER_CHROME") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    #[cfg(target_os = "macos")]
    let candidates = vec![
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary"),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
    ];

    #[cfg(target_os = "windows")]
    let candidates = {
        let mut candidates = Vec::new();
        for base in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(base) {
                candidates.push(PathBuf::from(&base).join("Google/Chrome/Application/chrome.exe"));
                candidates.push(PathBuf::from(&base).join("Chromium/Application/chrome.exe"));
            }
        }
        candidates
    };

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let candidates = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .flat_map(|dir| {
            [
                dir.join("google-chrome"),
                dir.join("google-chrome-stable"),
                dir.join("chromium"),
                dir.join("chromium-browser"),
            ]
        })
        .collect::<Vec<_>>();

    candidates.into_iter().find(|path| path.is_file())
}

fn run_browser_json(workspace: &Path, puffer_home: &Path, args: &[&str]) -> Value {
    let output = run_browser_output(workspace, puffer_home, args);
    assert!(
        output.status.success(),
        "browser command failed\nargs: {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "failed to parse browser JSON output: {error}\nstdout:\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn run_browser_output(workspace: &Path, puffer_home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_puffer"))
        .current_dir(workspace)
        .env("PUFFER_HOME", puffer_home)
        .arg("browser")
        .arg("--json")
        .args(args)
        .output()
        .expect("puffer browser process")
}

fn start_daemon(workspace: &Path, puffer_home: &Path, chrome: &Path) -> DaemonHandle {
    let handshake_path = ConfigPaths::discover(workspace)
        .workspace_config_dir
        .join("daemon.handshake");
    let mut child = Command::new(env!("CARGO_BIN_EXE_puffer"))
        .current_dir(workspace)
        .env("PUFFER_HOME", puffer_home)
        .env("PUFFER_CHROME", chrome)
        .arg("daemon")
        .arg("--bind")
        .arg("127.0.0.1:0")
        .arg("--handshake-file")
        .arg(&handshake_path)
        .arg("--print-handshake")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn puffer daemon");

    let stdout = child.stdout.take().expect("daemon stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("read daemon handshake line");
    if line.trim().is_empty() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("daemon failed to print a handshake line:\n{stderr}");
    }

    let handshake: Value = serde_json::from_str(line.trim()).expect("parse daemon handshake");
    assert!(
        handshake.get("url").and_then(Value::as_str).is_some(),
        "daemon handshake should include a websocket url: {handshake}"
    );

    DaemonHandle {
        child,
        workspace: workspace.to_path_buf(),
        puffer_home: puffer_home.to_path_buf(),
    }
}

fn wait_for_eval_string(
    workspace: &Path,
    puffer_home: &Path,
    script: &str,
    expected: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    let mut last = None;
    while Instant::now() < deadline {
        let response = run_browser_json(workspace, puffer_home, &["eval", script]);
        let value = response
            .pointer("/result/value")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if value.as_deref() == Some(expected) {
            return;
        }
        last = value;
        thread::sleep(Duration::from_millis(100));
    }

    panic!(
        "timed out waiting for browser eval `{script}` to become {expected:?}; last value was {:?}",
        last
    );
}
