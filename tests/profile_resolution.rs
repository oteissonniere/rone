//! Profile resolution end to end: an explicit single `--profile` must be
//! honoured (not silently replaced by the config default or the built-in
//! localhost fallback) and a name that no config file defines must fail
//! loudly instead of reaching some unexpected endpoint.

use std::path::PathBuf;
use std::process::{Command, Stdio};

const STATE: &str = "I was billed twice for the same invoice.";

/// A hermetic working directory with only `presets/triage.toml` (copied from
/// the repo), so neither a local `rone.toml`, nor the developer's user
/// config, nor any other preset can influence the outcome.
fn workspace_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rone-pr-{}-{name}", std::process::id()));
    std::fs::create_dir_all(dir.join("presets")).unwrap();
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("presets/triage.toml");
    std::fs::copy(&source, dir.join("presets/triage.toml")).unwrap();
    dir
}

/// Run the `rone` binary in a hermetic working directory with every config
/// source from the environment disabled.
fn run_rone(dir: &PathBuf, args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rone"));
    cmd.args(args)
        .current_dir(dir)
        // No config anywhere: not in the working directory either.
        .env_remove("RONE_CONFIG")
        .env_remove("RONE_PROFILE")
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .env_remove("XDG_CONFIG_HOME")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output().expect("run rone");
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (code, stdout, stderr)
}

/// `--profile X` without any config file: a clear error naming the profile,
/// not a silent request to the built-in localhost endpoint.
#[test]
fn ask_single_profile_without_config_fails_loudly() {
    let dir = workspace_dir("noconfig");
    let (code, stdout, stderr) = run_rone(
        &dir,
        &[
            "ask",
            "--preset",
            "triage",
            "--text",
            STATE,
            "--profile",
            "no-such-profile",
        ],
    );
    assert_ne!(code, 0, "stderr was: {stderr}");
    assert!(
        stderr.contains("unknown profile 'no-such-profile'"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("no rone.toml found"),
        "stderr was: {stderr}"
    );
    // The error must come before any request is attempted.
    assert!(!stdout.contains("POST "), "stdout was: {stdout}");
}

/// A single explicit `--profile` resolves to that profile's base URL, even
/// when the config also defines a default (dry-run: nothing is sent).
#[test]
fn ask_single_profile_uses_the_named_profile() {
    let dir = workspace_dir("named");
    let config_path = dir.join("config.toml");
    std::fs::write(
        &config_path,
        "default_profile = \"other\"\n\n[profiles.a]\nbase_url = \
         \"http://127.0.0.1:65531/v1\"\nmodel = \"mock-a\"\n\n[profiles.other]\n\
         base_url = \"http://127.0.0.1:65532/v1\"\nmodel = \"mock-b\"\n",
    )
    .unwrap();
    let (code, stdout, stderr) = run_rone(
        &dir,
        &[
            "ask",
            "--config",
            config_path.to_str().unwrap(),
            "--preset",
            "triage",
            "--text",
            STATE,
            "--profile",
            "a",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "stderr was: {stderr}");
    // The explicit profile wins over the config's default_profile.
    assert!(
        stdout.contains("POST http://127.0.0.1:65531/v1/systemone"),
        "stdout was: {stdout}"
    );
    assert!(!stdout.contains("65532"), "stdout was: {stdout}");
}

/// No `--profile`, no config file at all: the built-in default endpoint is
/// used (this fallback must keep working).
#[test]
fn ask_without_profile_and_config_uses_builtin_defaults() {
    let dir = workspace_dir("builtin");
    let (code, stdout, stderr) = run_rone(
        &dir,
        &["ask", "--preset", "triage", "--text", STATE, "--dry-run"],
    );
    assert_eq!(code, 0, "stderr was: {stderr}");
    assert!(
        stdout.contains("POST http://localhost:1234/v1/systemone"),
        "stdout was: {stdout}"
    );
}
