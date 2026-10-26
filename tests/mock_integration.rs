//! Integration tests: the `rone` CLI end to end against `mock_server`.
//!
//! Each test starts a mock server on a free local port, then invokes the
//! `rone` binary with explicit `--base-url`/`--model` flags and an empty
//! `--config` file, so no local `rone.toml` interferes. The mock answers are
//! a pure function of the input, so the expected values below are pinned
//! against the FNV-1a generator in `src/mock.rs`.

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

const STATE: &str = "I was billed twice for the same invoice.";
const STATE_2: &str = "My router keeps dropping the connection.";

/// A mock server child that is killed on drop, so a failed assert never
/// leaves a server listening.
struct ServerChild(std::process::Child);

impl Drop for ServerChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start `mock_server` on a free local port; returns its base URL and the
/// child process.
fn start_mock_server() -> (String, ServerChild) {
    let probe = TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    let port = probe.local_addr().expect("local addr").port();
    drop(probe);

    let child = Command::new(env!("CARGO_BIN_EXE_mock_server"))
        .env("RONE_MOCK_PORT", port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn mock_server");

    // Wait until the port accepts connections.
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "mock server did not start in time"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    (format!("http://{addr}/v1"), ServerChild(child))
}

/// A temp config file with no profiles, so the CLI never sees the local
/// `rone.toml` (which carries real server profiles).
fn empty_config() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rone-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("empty-config.toml");
    std::fs::write(&path, "# no profiles\n").unwrap();
    path
}

/// One state per line, written to a temp file for `rone batch --file`.
fn batch_input_file(name: &str, lines: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rone-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let mut content = String::new();
    for line in lines {
        content.push_str(line);
        content.push('\n');
    }
    std::fs::write(&path, content).unwrap();
    path
}

/// Run the `rone` binary and capture its exit code and output.
fn run_rone(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rone"))
        .args(args)
        .output()
        .expect("run rone");
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (code, stdout, stderr)
}

#[test]
fn ask_renders_answers_in_text_mode() {
    let (url, server) = start_mock_server();
    let config = empty_config();

    let (code, stdout, stderr) = run_rone(&[
        "--config",
        config.to_str().unwrap(),
        "ask",
        "--preset",
        "triage",
        "--text",
        STATE,
        "--base-url",
        &url,
        "--model",
        "mock",
    ]);

    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("department:"), "stdout: {stdout}");
    // The chosen option is marked and shown first, with its probability.
    assert!(stdout.contains("*sales"), "stdout: {stdout}");
    assert!(stdout.contains("97.70%"), "stdout: {stdout}");
    assert!(stdout.contains("needs_human:"), "stdout: {stdout}");
    assert!(stdout.contains("76.10%"), "stdout: {stdout}");
    drop(server);
}

#[test]
fn ask_json_matches_the_pinned_mock_values() {
    let (url, server) = start_mock_server();
    let config = empty_config();

    let (code, stdout, stderr) = run_rone(&[
        "--config",
        config.to_str().unwrap(),
        "ask",
        "--preset",
        "triage",
        "--text",
        STATE,
        "--base-url",
        &url,
        "--model",
        "mock",
        "--json",
    ]);

    assert_eq!(code, 0, "stderr: {stderr}");
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    let dept = &parsed["department"];
    assert_eq!(dept["choice"], "sales");
    let confidence = dept["confidence"].as_f64().unwrap();
    assert!((confidence - 0.977).abs() < 1e-9);
    let probs = dept["probabilities"].as_object().unwrap();
    assert!((probs["sales"].as_f64().unwrap() - 0.977).abs() < 1e-9);
    assert!((probs["billing"].as_f64().unwrap() - 0.0115).abs() < 1e-9);
    assert!((probs["technical"].as_f64().unwrap() - 0.0115).abs() < 1e-9);
    let noul = parsed["needs_human"]["noul"].as_f64().unwrap();
    assert!((noul - 0.761).abs() < 1e-9);
    drop(server);
}

#[test]
fn ask_below_threshold_exits_3_with_a_warning() {
    let (url, server) = start_mock_server();
    let config = empty_config();

    let (code, stdout, stderr) = run_rone(&[
        "--config",
        config.to_str().unwrap(),
        "ask",
        "--preset",
        "triage",
        "--text",
        STATE,
        "--base-url",
        &url,
        "--model",
        "mock",
        "--min-confidence",
        "0.9",
    ]);

    // Answers are still printed, but the process signals the low confidence.
    assert_eq!(code, 3, "stderr: {stderr}");
    assert!(stdout.contains("*sales"), "stdout: {stdout}");
    // `department` (0.977) passes; `needs_human` (0.761) is flagged.
    assert!(stderr.contains("needs_human"), "stderr: {stderr}");
    assert!(stderr.contains("0.76"), "stderr: {stderr}");
    assert!(!stderr.contains("'department'"), "stderr: {stderr}");
    drop(server);
}

#[test]
fn batch_renders_numbered_blocks_in_text_mode() {
    let (url, server) = start_mock_server();
    let config = empty_config();
    let file = batch_input_file("inputs-text.txt", &[STATE, STATE_2]);

    let (code, stdout, stderr) = run_rone(&[
        "--config",
        config.to_str().unwrap(),
        "batch",
        "--preset",
        "triage",
        "--file",
        file.to_str().unwrap(),
        "--base-url",
        &url,
        "--model",
        "mock",
    ]);

    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.starts_with(&format!("#1: {STATE}\n")),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&format!("#2: {STATE_2}")),
        "stdout: {stdout}"
    );
    drop(server);
}

#[test]
fn batch_json_exits_3_when_any_answer_is_below_threshold() {
    let (url, server) = start_mock_server();
    let config = empty_config();
    let file = batch_input_file("inputs-json.txt", &[STATE, STATE_2]);

    let (code, stdout, stderr) = run_rone(&[
        "--config",
        config.to_str().unwrap(),
        "batch",
        "--preset",
        "triage",
        "--file",
        file.to_str().unwrap(),
        "--base-url",
        &url,
        "--model",
        "mock",
        "--json",
        "--min-confidence",
        "0.9",
    ]);

    // The noul answer is always below 0.9 with the mock generator, so both
    // inputs are flagged.
    assert_eq!(code, 3, "stderr: {stderr}");
    let arr: Vec<Value> = serde_json::from_str(&stdout).expect("stdout is a JSON array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["input"], STATE);
    assert!(arr[0]["answers"].get("department").is_some());
    assert_eq!(arr[1]["input"], STATE_2);
    assert!(arr[1]["answers"].get("needs_human").is_some());
    assert!(stderr.contains("input #1"), "stderr: {stderr}");
    assert!(stderr.contains("input #2"), "stderr: {stderr}");
    drop(server);
}
