use std::collections::BTreeMap;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::Value;

use rone::color::ColorMode;
use rone::config::{
    Config, Profile, load_config, parse_profiles, profile_from_env, render_profiles,
    resolve_api_key,
};
use rone::{
    EvalResult, InstallKind, ModelList, below_threshold, build_request, exe_install_kind,
    extract_answers, format_duration, list_presets, load_eval_cases, load_preset, load_presets,
    parse_eval_cases, parse_vars, preset_dirs, read_state, read_states, render_answers,
    render_batch, render_comparison, render_eval_report, render_latency, render_models,
    render_presets, validate,
};

mod mcp;

/// Rone: makes System One decision models pleasant to use from the command line.
#[derive(Parser)]
#[command(name = "rone", version, about)]
struct Cli {
    /// Path to the config file (defaults to rone.toml, then ~/.config/rone/rone.toml)
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Disable colored output (colors are on by default on a terminal; the
    /// NO_COLOR environment variable is also honored)
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Ask a System One model using a reusable preset.
    Ask {
        /// Preset name, loaded from presets/<name>.toml (comma-separated list to merge
        /// several presets into one request, e.g. `a,b`)
        #[arg(long, value_name = "NAME[,NAME]")]
        preset: String,

        /// Input text (use --file or stdin for longer input)
        #[arg(short, long)]
        text: Option<String>,

        /// Read the input text from a file
        #[arg(short, long, value_name = "PATH")]
        file: Option<PathBuf>,

        /// Profile from the config file (comma-separated list to compare several
        /// profiles, e.g. `a,b`; sets base_url, model, min_confidence, api_key, timeout)
        #[arg(long, value_name = "NAME[,NAME]")]
        profile: Option<String>,

        /// Base URL of the /v1/systemone-compatible endpoint (overrides the profile)
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,

        /// Model name sent in the request (overrides the profile)
        #[arg(long, value_name = "NAME")]
        model: Option<String>,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,

        /// Print the exact request that would be sent, without sending it
        #[arg(long)]
        dry_run: bool,

        /// Only accept answers with at least this much confidence (0..=1)
        #[arg(long, value_parser = parse_confidence, value_name = "F")]
        min_confidence: Option<f64>,

        /// API key sent as a Bearer token (overrides the profile; `env:VAR` reads from the environment)
        #[arg(long, value_name = "KEY")]
        api_key: Option<String>,

        /// Request timeout in seconds (overrides the profile)
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,

        /// Variable injected into question instructions as `{{name}}` (repeatable)
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
    },
    /// Ask a System One model over many inputs (one state per line).
    Batch {
        /// Preset name, loaded from presets/<name>.toml
        #[arg(long)]
        preset: String,

        /// Read one state per line from a file (or stdin)
        #[arg(short, long, value_name = "PATH")]
        file: Option<PathBuf>,

        /// Profile from the config file (sets base_url, model, min_confidence, api_key, timeout)
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

        /// Base URL of the /v1/systemone-compatible endpoint (overrides the profile)
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,

        /// Model name sent in the request (overrides the profile)
        #[arg(long, value_name = "NAME")]
        model: Option<String>,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,

        /// Only accept answers with at least this much confidence (0..=1)
        #[arg(long, value_parser = parse_confidence, value_name = "F")]
        min_confidence: Option<f64>,

        /// API key sent as a Bearer token (overrides the profile; `env:VAR` reads from the environment)
        #[arg(long, value_name = "KEY")]
        api_key: Option<String>,

        /// Request timeout in seconds (overrides the profile)
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,

        /// Variable injected into question instructions as `{{name}}` (repeatable)
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
    },
    /// Evaluate a preset over a dataset of expected answers (a JSON array of
    /// {"input", "expected"} objects).
    Eval {
        /// Preset name, loaded from presets/<name>.toml
        #[arg(long)]
        preset: String,

        /// Path to the eval dataset (a JSON array of {"input", "expected"} objects, or stdin)
        #[arg(short, long, value_name = "PATH")]
        file: Option<PathBuf>,

        /// Profile from the config file (sets base_url, model, api_key, timeout)
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

        /// Base URL of the /v1/systemone-compatible endpoint (overrides the profile)
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,

        /// Model name sent in the request (overrides the profile)
        #[arg(long, value_name = "NAME")]
        model: Option<String>,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,

        /// API key sent as a Bearer token (overrides the profile; `env:VAR` reads from the environment)
        #[arg(long, value_name = "KEY")]
        api_key: Option<String>,

        /// Request timeout in seconds (overrides the profile)
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,

        /// Variable injected into question instructions as `{{name}}` (repeatable)
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
    },
    /// List the presets available in the presets/ directory.
    ListPresets,
    /// List the profiles defined in the config file.
    ListProfiles,
    /// Run rone as an MCP (Model Context Protocol) server over stdio.
    Mcp,
    /// List the models available on the configured endpoint (GET /v1/models).
    Models {
        /// Profile from the config file (sets base_url, api_key, timeout)
        #[arg(long, value_name = "NAME")]
        profile: Option<String>,

        /// Base URL of the endpoint (overrides the profile)
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,
    },
    /// Upgrade rone itself: re-runs the installer script when rone was
    /// installed with it, or points to `brew upgrade` for a Homebrew install.
    Upgrade,
}

/// The one-liner that installs or upgrades rone from the GitHub releases.
const INSTALL_SCRIPT_ONE_LINER: &str =
    "curl -fsSL https://raw.githubusercontent.com/oteissonniere/rone/main/scripts/install.sh | sh";

/// Parse a confidence threshold between 0 and 1 (clap value parser).
fn parse_confidence(value: &str) -> Result<f64, String> {
    let v: f64 = value
        .parse()
        .map_err(|_| format!("'{value}' is not a number"))?;
    if !(0.0..=1.0).contains(&v) {
        return Err(format!("'{value}' must be between 0 and 1"));
    }
    Ok(v)
}

/// The effective request target after merging explicit CLI flags, the
/// selected profile, and built-in defaults.
struct Target {
    base_url: String,
    model: String,
    min_confidence: Option<f64>,
    api_key: Option<String>,
    timeout: Option<u64>,
}

/// Resolve the effective target: explicit command-line flags win, then the
/// selected profile, then built-in defaults.
fn resolve_target(
    config: Option<&Config>,
    profile: Option<&str>,
    base_url: Option<&str>,
    model: Option<&str>,
    min_confidence: Option<f64>,
    api_key: Option<&str>,
    timeout: Option<u64>,
) -> Result<Target> {
    let profile = match (config, profile) {
        (Some(cfg), _) => cfg.resolve_profile(profile)?,
        // Without a config file only the built-in default may apply; an
        // explicitly requested profile is a clear error, not a silent fallback.
        (None, Some(name)) => bail!(
            "unknown profile '{name}': no rone.toml found (looked for \
             $RONE_CONFIG, ./rone.toml and ~/.config/rone/rone.toml)"
        ),
        (None, None) => Profile::default(),
    };
    let api_key = match api_key {
        Some(k) => Some(k.to_string()),
        None => profile
            .api_key
            .as_deref()
            .map(resolve_api_key)
            .transpose()?,
    };
    Ok(Target {
        base_url: base_url.map(str::to_string).unwrap_or(profile.base_url),
        model: model.map(str::to_string).unwrap_or(profile.model),
        min_confidence: min_confidence.or(profile.min_confidence),
        api_key,
        timeout: timeout.or(profile.timeout),
    })
}

/// The full URL of the /v1/systemone endpoint for a base URL.
fn request_url(base_url: &str) -> String {
    format!("{}/systemone", base_url.trim_end_matches('/'))
}

async fn send(
    base_url: &str,
    request: &Value,
    api_key: Option<&str>,
    timeout: Option<u64>,
) -> Result<Value> {
    let url = request_url(base_url);
    let mut builder = reqwest::Client::builder();
    if let Some(t) = timeout {
        builder = builder.timeout(Duration::from_secs(t));
    }
    let client = builder.build().context("failed to build HTTP client")?;
    let mut req = client.post(&url).json(request);
    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("failed to reach {url}"))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .with_context(|| format!("invalid JSON response from {url}"))?;
    if !status.is_success() {
        bail!("server returned {status}: {body}");
    }
    Ok(body)
}

/// GET a JSON document, adding a Bearer header when an API key is set.
async fn get_json<T: serde::de::DeserializeOwned>(
    url: &str,
    api_key: Option<&str>,
    timeout: Option<u64>,
) -> Result<T> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout.unwrap_or(30)))
        .build()?;
    let mut request = client.get(url);
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        bail!("server returned {status}");
    }
    Ok(response.json().await?)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config;
    let config = load_config(config_path.as_deref())?;
    // Colors are on by default on a terminal; off when stdout is piped, when
    // NO_COLOR is set, or when --no-color is given.
    let colors = ColorMode::new(
        !cli.no_color && std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    );
    match cli.command {
        Command::Ask {
            preset,
            text,
            file,
            profile,
            base_url,
            model,
            json,
            dry_run,
            min_confidence,
            api_key,
            timeout,
            vars,
        } => {
            // The RONE_PROFILE environment variable is a fallback for
            // --profile: an explicit flag always wins.
            let profile = profile.or_else(profile_from_env);
            // `--profile a,b` compares the same preset over several
            // profiles; a single profile (or none) keeps the plain output.
            let profile_names = profile.as_deref().map(parse_profiles).unwrap_or_default();
            let comparing = profile_names.len() > 1;
            let names: Vec<Option<&str>> = if comparing {
                profile_names.iter().map(String::as_str).map(Some).collect()
            } else if let Some(name) = profile_names.first() {
                // A single explicit --profile (or RONE_PROFILE): use it as-is.
                vec![Some(name.as_str())]
            } else {
                // None: the config's default profile (or built-in defaults).
                vec![None]
            };
            // `--preset a,b` merges the question sets of several presets into
            // one request; identical shared questions are deduplicated.
            let preset_names: Vec<String> = preset
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect();
            let preset = load_presets(&preset_names)?;
            validate(&preset)?;
            let state = read_state(&text, &file)?;
            let vars = parse_vars(&vars)?;

            let mut results: Vec<(String, String, BTreeMap<String, Value>, Duration)> = Vec::new();
            let mut any_below = false;
            let mut dry_printed = false;
            for name in &names {
                let label = name
                    .map(str::to_string)
                    .unwrap_or_else(|| "(default)".to_string());
                let target = resolve_target(
                    config.as_ref(),
                    *name,
                    base_url.as_deref(),
                    model.as_deref(),
                    min_confidence,
                    api_key.as_deref(),
                    timeout,
                )?;
                let request = build_request(&target.model, &state, &preset, &vars)?;
                if dry_run {
                    // Show the exact request without sending it.
                    if dry_printed {
                        println!();
                    }
                    println!("POST {}", request_url(&target.base_url));
                    println!("{}", serde_json::to_string_pretty(&request)?);
                    dry_printed = true;
                    continue;
                }
                let started = Instant::now();
                let response = send(
                    &target.base_url,
                    &request,
                    target.api_key.as_deref(),
                    target.timeout,
                )
                .await?;
                let elapsed = started.elapsed();
                let answers = extract_answers(&response);
                // Warnings carry the profile name in comparison mode; a
                // single profile keeps the plain message.
                let prefix = if comparing {
                    format!("profile '{label}': ")
                } else {
                    String::new()
                };
                if let Some(min) = target.min_confidence {
                    for (id, conf) in &below_threshold(&answers, min) {
                        any_below = true;
                        match conf {
                            Some(c) => eprintln!(
                                "warning: {prefix}answer '{id}': confidence {c:.2} is below threshold {min:.2}"
                            ),
                            None => eprintln!(
                                "warning: {prefix}answer '{id}': no confidence reported, threshold is {min:.2}"
                            ),
                        }
                    }
                }
                results.push((label, target.model.clone(), answers, elapsed));
            }
            if dry_run {
                // The requests were printed above; nothing was sent.
                return Ok(());
            }
            if comparing {
                println!("{}", render_comparison(&results, json, colors));
            } else {
                let mut out = render_answers(&results[0].2, json, colors);
                if !json {
                    out.push('\n');
                    out.push_str(&render_latency(results[0].3));
                }
                println!("{out}");
            }
            if any_below {
                // Exit code 3: answers received, but below the confidence threshold.
                std::process::exit(3);
            }
        }
        Command::Batch {
            preset,
            file,
            profile,
            base_url,
            model,
            json,
            min_confidence,
            api_key,
            timeout,
            vars,
        } => {
            // The RONE_PROFILE environment variable is a fallback for
            // --profile: an explicit flag always wins.
            let profile = profile.or_else(profile_from_env);
            let target = resolve_target(
                config.as_ref(),
                profile.as_deref(),
                base_url.as_deref(),
                model.as_deref(),
                min_confidence,
                api_key.as_deref(),
                timeout,
            )?;
            let preset = load_preset(&preset)?;
            validate(&preset)?;
            let states = read_states(&file)?;
            let vars = parse_vars(&vars)?;
            let mut results: Vec<(String, BTreeMap<String, Value>, Duration)> = Vec::new();
            let mut any_below = false;
            let total_started = Instant::now();
            for (i, state) in states.iter().enumerate() {
                let request = build_request(&target.model, state, &preset, &vars)?;
                let started = Instant::now();
                let response = send(
                    &target.base_url,
                    &request,
                    target.api_key.as_deref(),
                    target.timeout,
                )
                .await?;
                let elapsed = started.elapsed();
                let answers = extract_answers(&response);
                if let Some(min) = target.min_confidence {
                    for (id, conf) in &below_threshold(&answers, min) {
                        any_below = true;
                        match conf {
                            Some(c) => eprintln!(
                                "warning: input #{}: answer '{id}': confidence {c:.2} is below threshold {min:.2}",
                                i + 1
                            ),
                            None => eprintln!(
                                "warning: input #{}: answer '{id}': no confidence reported, threshold is {min:.2}",
                                i + 1
                            ),
                        }
                    }
                }
                results.push((state.clone(), answers, elapsed));
            }
            println!("{}", render_batch(&results, json, colors));
            // The total covers the whole batch, measured around the loop.
            if !json {
                println!("total: {}", format_duration(total_started.elapsed()));
            }
            if any_below {
                // Exit code 3: answers received, but at least one is below the
                // confidence threshold.
                std::process::exit(3);
            }
        }
        Command::Eval {
            preset,
            file,
            profile,
            base_url,
            model,
            json,
            api_key,
            timeout,
            vars,
        } => {
            // The RONE_PROFILE environment variable is a fallback for
            // --profile: an explicit flag always wins.
            let profile = profile.or_else(profile_from_env);
            let target = resolve_target(
                config.as_ref(),
                profile.as_deref(),
                base_url.as_deref(),
                model.as_deref(),
                None,
                api_key.as_deref(),
                timeout,
            )?;
            let preset = load_preset(&preset)?;
            validate(&preset)?;
            let cases = match &file {
                Some(f) => load_eval_cases(f)?,
                None => {
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .context("failed to read stdin")?;
                    parse_eval_cases(&buf)?
                }
            };
            let vars = parse_vars(&vars)?;
            let mut results = Vec::new();
            for case in &cases {
                let request = build_request(&target.model, &case.input, &preset, &vars)?;
                let response = send(
                    &target.base_url,
                    &request,
                    target.api_key.as_deref(),
                    target.timeout,
                )
                .await?;
                let answers = extract_answers(&response);
                results.push(EvalResult {
                    input: case.input.clone(),
                    expected: case.expected.clone(),
                    answers,
                });
            }
            println!("{}", render_eval_report(&results, json, colors));
        }
        Command::ListPresets => {
            if preset_dirs().is_empty() {
                eprintln!(
                    "no presets/ directory found (looked in the working directory, \
                     the user config directory and next to the executable)"
                );
            } else {
                let names = list_presets()?;
                if names.is_empty() {
                    eprintln!("no presets found in the presets/ directories");
                } else {
                    println!("{}", render_presets(&names, colors));
                }
            }
        }
        Command::ListProfiles => match &config {
            Some(cfg) if !cfg.profiles.is_empty() => {
                println!("{}", render_profiles(cfg, colors));
            }
            Some(_) => eprintln!("no profiles defined in the config file"),
            None => eprintln!(
                "no config file found (looked for rone.toml and ~/.config/rone/rone.toml)"
            ),
        },
        Command::Mcp => mcp::run().await?,
        Command::Models { profile, base_url } => {
            let profile = profile.or_else(profile_from_env);
            let target = resolve_target(
                config.as_ref(),
                profile.as_deref(),
                base_url.as_deref(),
                None,
                None,
                None,
                None,
            )?;
            let url = format!("{}/models", target.base_url.trim_end_matches('/'));
            let list: ModelList = get_json(&url, target.api_key.as_deref(), target.timeout).await?;
            println!("{}", render_models(&list.data, colors));
        }
        Command::Upgrade => match exe_install_kind() {
            Some(InstallKind::Brew) => {
                // Homebrew manages its own upgrade path (the tap is refreshed
                // on every brew run), so only point at the brew command.
                println!("rone is installed via Homebrew; upgrade it with:\n  brew upgrade rone");
            }
            Some(InstallKind::Script) => {
                println!("Fetching and running the installer script (latest release)...");
                let status = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(INSTALL_SCRIPT_ONE_LINER)
                    .status()
                    .with_context(|| "failed to run the installer script")?;
                if !status.success() {
                    bail!(
                        "the installer script failed (exit code {})",
                        status.code().unwrap_or(-1),
                    );
                }
                println!("done: rone upgraded by the installer script");
            }
            // A binary run from somewhere rone does not recognize (e.g. a
            // source build under target/): show both upgrade paths and stop.
            None => {
                eprintln!(
                    "could not detect how rone was installed (running from an unrecognized location)"
                );
                eprintln!("upgrade it manually with one of:");
                eprintln!("  - installer script: {INSTALL_SCRIPT_ONE_LINER}");
                eprintln!("  - Homebrew (macOS): brew upgrade rone");
                std::process::exit(2);
            }
        },
    }
    Ok(())
}
