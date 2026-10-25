use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use serde_json::{json, Value};

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
    },
}

/// A preset file: `presets/<name>.toml`.
#[derive(Deserialize)]
struct Preset {
    questions: BTreeMap<String, QuestionDef>,
}

#[derive(Deserialize)]
struct QuestionDef {
    #[serde(rename = "type")]
    kind: QuestionType,
    instructions: String,
    #[serde(default)]
    options: Vec<String>,
}

#[derive(Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
enum QuestionType {
    Choice,
    Score,
    Noul,
}

impl QuestionType {
    fn as_str(self) -> &'static str {
        match self {
            QuestionType::Choice => "choice",
            QuestionType::Score => "score",
            QuestionType::Noul => "noul",
        }
    }
}

fn load_preset(name: &str) -> Result<Preset> {
    let path = PathBuf::from("presets").join(format!("{name}.toml"));
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read preset file {}", path.display()))?;
    let preset: Preset = toml::from_str(&content)
        .with_context(|| format!("failed to parse preset {}", path.display()))?;
    if preset.questions.is_empty() {
        bail!("preset '{}' defines no questions", name);
    }
    Ok(preset)
}

/// Catch common mistakes before sending, with clear messages.
fn validate(preset: &Preset) -> Result<()> {
    for (id, q) in &preset.questions {
        if q.kind == QuestionType::Choice {
            if q.options.is_empty() {
                bail!(
                    "question '{id}': a choice question needs at least one option"
                );
            }
            if q.options.len() > 10 {
                eprintln!(
                    "warning: question '{id}' has {} options in a single choice; \
                     this may degrade results",
                    q.options.len()
                );
            }
        }
    }
    Ok(())
}

/// Input text comes from --text, --file, or stdin (in that order).
fn read_state(text: &Option<String>, file: &Option<PathBuf>) -> Result<String> {
    let raw = if let Some(t) = text {
        t.clone()
    } else if let Some(f) = file {
        std::fs::read_to_string(f)
            .with_context(|| format!("failed to read file {}", f.display()))?
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read stdin")?;
        buf
    };
    let state = raw.trim().to_string();
    if state.is_empty() {
        bail!("empty input: provide text with --text, --file, or stdin");
    }
    Ok(state)
}

/// Build the `/v1/systemone` request body.
fn build_request(model: &str, state: &str, preset: &Preset) -> Value {
    let mut questions = serde_json::Map::new();
    for (id, q) in &preset.questions {
        let mut qval = json!({
            "type": q.kind.as_str(),
            "instructions": q.instructions,
        });
        if q.kind == QuestionType::Choice {
            // The endpoint expects `criteria` as an object (option -> description).
            let mut criteria = serde_json::Map::new();
            for opt in &q.options {
                criteria.insert(opt.clone(), json!(opt));
            }
            qval["criteria"] = Value::Object(criteria);
        }
        questions.insert(id.clone(), qval);
    }
    json!({
        "model": model,
        "state": state,
        "questions": questions,
    })
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

fn extract_answers(response: &Value) -> BTreeMap<String, Value> {
    response
        .get("answers")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

fn print_answers(answers: &BTreeMap<String, Value>, as_json: bool) {
    if as_json {
        println!("{}", serde_json::to_string_pretty(answers).unwrap_or_default());
        return;
    }
    for (id, answer) in answers {
        println!("{id}:");
        // choice: `probabilities` map + `choice` for the picked option
        if let Some(probs) = answer.get("probabilities").and_then(Value::as_object) {
            let mut entries: Vec<(String, f64)> = probs
                .iter()
                .filter_map(|(k, v)| v.as_f64().map(|p| (k.clone(), p)))
                .collect();
            entries.sort_by(|a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
            let chosen = answer.get("choice").and_then(Value::as_str);
            for (name, prob) in &entries {
                let marker = if chosen.as_deref() == Some(name.as_str()) {
                    "*"
                } else {
                    " "
                };
                println!("  {marker}{name:<24} {:>7.2}%", prob * 100.0);
            }
        } else if let Some(prob) = answer.get("noul").and_then(Value::as_f64) {
            println!("  yes: {:>8.2}%", prob * 100.0);
        } else if let Some(prob) = answer.get("prob").and_then(Value::as_f64) {
            println!("  {:>8.2}%", prob * 100.0);
        } else {
            println!("  {answer}");
        }
    }
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
        } => {
            let preset = load_preset(&preset)?;
            validate(&preset)?;
            let state = read_state(&text, &file)?;
            let request = build_request(&model, &state, &preset);
            let response = send(&base_url, &request).await?;
            let answers = extract_answers(&response);
            print_answers(&answers, json);
        }
    }
    Ok(())
}
