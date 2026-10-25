use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::Value;

use rone::{
    below_threshold, build_request, extract_answers, load_preset, parse_vars, read_state,
    read_states, render_answers, render_batch, validate,
};

/// Rone: makes System One decision models pleasant to use from the command line.
#[derive(Parser)]
#[command(name = "rone", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Ask a System One model using a reusable preset.
    Ask {
        /// Preset name, loaded from presets/<name>.toml
        #[arg(long)]
        preset: String,

        /// Input text (use --file or stdin for longer input)
        #[arg(short, long)]
        text: Option<String>,

        /// Read the input text from a file
        #[arg(short, long, value_name = "PATH")]
        file: Option<PathBuf>,

        /// Base URL of the /v1/systemone-compatible endpoint
        #[arg(long, default_value = "http://localhost:1234/v1")]
        base_url: String,

        /// Model name sent in the request
        #[arg(long, default_value = "laya")]
        model: String,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,

        /// Only accept answers with at least this much confidence (0..=1)
        #[arg(long, value_parser = parse_confidence, value_name = "F")]
        min_confidence: Option<f64>,

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

        /// Base URL of the /v1/systemone-compatible endpoint
        #[arg(long, default_value = "http://localhost:1234/v1")]
        base_url: String,

        /// Model name sent in the request
        #[arg(long, default_value = "laya")]
        model: String,

        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,

        /// Only accept answers with at least this much confidence (0..=1)
        #[arg(long, value_parser = parse_confidence, value_name = "F")]
        min_confidence: Option<f64>,

        /// Variable injected into question instructions as `{{name}}` (repeatable)
        #[arg(long = "var", value_name = "NAME=VALUE")]
        vars: Vec<String>,
    },
}

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

async fn send(base_url: &str, request: &Value) -> Result<Value> {
    let url = format!("{}/systemone", base_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(request)
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Ask {
            preset,
            text,
            file,
            base_url,
            model,
            json,
            min_confidence,
            vars,
        } => {
            let preset = load_preset(&preset)?;
            validate(&preset)?;
            let state = read_state(&text, &file)?;
            let vars = parse_vars(&vars)?;
            let request = build_request(&model, &state, &preset, &vars)?;
            let response = send(&base_url, &request).await?;
            let answers = extract_answers(&response);
            println!("{}", render_answers(&answers, json));
            if let Some(min) = min_confidence {
                let failed = below_threshold(&answers, min);
                for (id, conf) in &failed {
                    match conf {
                        Some(c) => eprintln!(
                            "warning: answer '{id}': confidence {c:.2} is below threshold {min:.2}"
                        ),
                        None => eprintln!(
                            "warning: answer '{id}': no confidence reported, threshold is {min:.2}"
                        ),
                    }
                }
                if !failed.is_empty() {
                    // Exit code 3: answers received, but below the confidence threshold.
                    std::process::exit(3);
                }
            }
        }
        Command::Batch {
            preset,
            file,
            base_url,
            model,
            json,
            min_confidence,
            vars,
        } => {
            let preset = load_preset(&preset)?;
            validate(&preset)?;
            let states = read_states(&file)?;
            let vars = parse_vars(&vars)?;
            let mut results: Vec<(String, BTreeMap<String, Value>)> = Vec::new();
            let mut any_below = false;
            for (i, state) in states.iter().enumerate() {
                let request = build_request(&model, state, &preset, &vars)?;
                let response = send(&base_url, &request).await?;
                let answers = extract_answers(&response);
                if let Some(min) = min_confidence {
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
                results.push((state.clone(), answers));
            }
            println!("{}", render_batch(&results, json));
            if any_below {
                // Exit code 3: answers received, but at least one is below the
                // confidence threshold.
                std::process::exit(3);
            }
        }
    }
    Ok(())
}
