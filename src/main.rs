use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use serde_json::{Value, json};

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
    /// Optional option → description, sent as `criteria` for `choice`
    /// questions. Options without a description use their own name.
    #[serde(default)]
    criteria: BTreeMap<String, String>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
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
    let preset =
        parse_preset(&content).with_context(|| format!("in preset file {}", path.display()))?;
    Ok(preset)
}

fn parse_preset(content: &str) -> Result<Preset> {
    let preset: Preset = toml::from_str(content).context("failed to parse preset TOML")?;
    if preset.questions.is_empty() {
        bail!("preset defines no questions");
    }
    Ok(preset)
}

/// Catch common mistakes before sending, with clear messages.
fn validate(preset: &Preset) -> Result<()> {
    for (id, q) in &preset.questions {
        if q.kind == QuestionType::Choice {
            if q.options.is_empty() {
                bail!("question '{id}': a choice question needs at least one option");
            }
            for option in q.criteria.keys() {
                if !q.options.contains(option) {
                    bail!("question '{id}': criteria entry for unknown option '{option}'");
                }
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

/// Parse `--var NAME=VALUE` pairs into an ordered map. The value may contain
/// further `=` characters; the split happens at the first one.
fn parse_vars(vars: &[String]) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for var in vars {
        let (name, value) = var
            .split_once('=')
            .with_context(|| format!("invalid variable '{var}': expected NAME=VALUE"))?;
        if name.is_empty() {
            bail!("invalid variable '{var}': the name must not be empty");
        }
        map.insert(name.to_string(), value.to_string());
    }
    Ok(map)
}

/// Replace `{{name}}` placeholders in `text` with the given variables.
/// An unknown placeholder is an error: better to fail than to send it to the
/// model.
fn substitute(text: &str, vars: &BTreeMap<String, String>) -> Result<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let end = after_open
            .find("}}")
            .with_context(|| "unterminated '{{' placeholder in question instructions")?;
        let name = after_open[..end].trim();
        let value = vars
            .get(name)
            .with_context(|| format!("unknown variable '{{{name}}}' in question instructions"))?;
        out.push_str(value);
        rest = &after_open[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Build the `/v1/systemone` request body. `vars` are substituted into the
/// instructions of every question.
fn build_request(
    model: &str,
    state: &str,
    preset: &Preset,
    vars: &BTreeMap<String, String>,
) -> Result<Value> {
    let mut questions = serde_json::Map::new();
    for (id, q) in &preset.questions {
        let instructions =
            substitute(&q.instructions, vars).with_context(|| format!("in question '{id}'"))?;
        let mut qval = json!({
            "type": q.kind.as_str(),
            "instructions": instructions,
        });
        if q.kind == QuestionType::Choice {
            // The endpoint expects `criteria` as an object (option -> description).
            let mut criteria = serde_json::Map::new();
            for opt in &q.options {
                // An option without a description is described by its own name.
                let description = q.criteria.get(opt).cloned().unwrap_or_else(|| opt.clone());
                criteria.insert(opt.clone(), json!(description));
            }
            qval["criteria"] = Value::Object(criteria);
        }
        questions.insert(id.clone(), qval);
    }
    Ok(json!({
        "model": model,
        "state": state,
        "questions": questions,
    }))
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

/// The confidence an answer carries: its `confidence` field for `choice`
/// questions, falling back to `noul` and `prob`.
fn answer_confidence(answer: &Value) -> Option<f64> {
    answer
        .get("confidence")
        .or_else(|| answer.get("noul"))
        .or_else(|| answer.get("prob"))
        .and_then(Value::as_f64)
}

/// The answers whose confidence is below `min`, or missing (in preset order).
fn below_threshold(answers: &BTreeMap<String, Value>, min: f64) -> Vec<(String, Option<f64>)> {
    answers
        .iter()
        .filter_map(|(id, answer)| {
            let conf = answer_confidence(answer);
            match conf {
                Some(c) if c >= min => None,
                _ => Some((id.clone(), conf)),
            }
        })
        .collect()
}

fn render_answers(answers: &BTreeMap<String, Value>, as_json: bool) -> String {
    if as_json {
        return serde_json::to_string_pretty(answers).unwrap_or_default();
    }
    let mut out = String::new();
    for (id, answer) in answers {
        out.push_str(&format!("{id}:\n"));
        // choice: `probabilities` map + `choice` for the picked option
        if let Some(probs) = answer.get("probabilities").and_then(Value::as_object) {
            let mut entries: Vec<(String, f64)> = probs
                .iter()
                .filter_map(|(k, v)| v.as_f64().map(|p| (k.clone(), p)))
                .collect();
            entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let chosen = answer.get("choice").and_then(Value::as_str);
            for (name, prob) in &entries {
                let marker = if chosen == Some(name.as_str()) {
                    "*"
                } else {
                    " "
                };
                out.push_str(&format!("  {marker}{name:<24} {:>7.2}%\n", prob * 100.0));
            }
        } else if let Some(prob) = answer.get("noul").and_then(Value::as_f64) {
            out.push_str(&format!("  yes: {:>8.2}%\n", prob * 100.0));
        } else if let Some(prob) = answer.get("prob").and_then(Value::as_f64) {
            out.push_str(&format!("  {:>8.2}%\n", prob * 100.0));
        } else {
            out.push_str(&format!("  {answer}\n"));
        }
    }
    out
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(kind: QuestionType, options: Vec<&str>) -> QuestionDef {
        question_with_criteria(kind, options, &[])
    }

    fn question_with_criteria(
        kind: QuestionType,
        options: Vec<&str>,
        criteria: &[(&str, &str)],
    ) -> QuestionDef {
        QuestionDef {
            kind,
            instructions: "Pick one.".to_string(),
            options: options.into_iter().map(String::from).collect(),
            criteria: criteria
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn preset_of(questions: Vec<(&str, QuestionDef)>) -> Preset {
        Preset {
            questions: questions
                .into_iter()
                .map(|(id, q)| (id.to_string(), q))
                .collect(),
        }
    }

    #[test]
    fn parse_preset_parses_questions() {
        let preset = parse_preset(
            r#"
[questions.department]
type = "choice"
instructions = "Who should handle this?"
options = ["billing", "technical", "sales"]

[questions.department.criteria]
billing = "billing issues"
technical = "technical issues"
sales = "sales issues"

[questions.needs_human]
type = "noul"
instructions = "A human must handle this."
"#,
        )
        .unwrap();

        assert_eq!(preset.questions.len(), 2);
        let dept = &preset.questions["department"];
        assert_eq!(dept.kind, QuestionType::Choice);
        assert_eq!(dept.options, vec!["billing", "technical", "sales"]);
        assert_eq!(dept.criteria["billing"], "billing issues");
        assert_eq!(preset.questions["needs_human"].kind, QuestionType::Noul);
    }

    #[test]
    fn parse_preset_rejects_empty() {
        assert!(parse_preset("# no questions here").is_err());
    }

    #[test]
    fn validate_rejects_empty_choice() {
        let preset = preset_of(vec![("a", question(QuestionType::Choice, vec![]))]);
        assert!(validate(&preset).is_err());
    }

    #[test]
    fn validate_accepts_choice_with_options() {
        let preset = preset_of(vec![("a", question(QuestionType::Choice, vec!["x", "y"]))]);
        assert!(validate(&preset).is_ok());
    }

    #[test]
    fn validate_ignores_options_for_noul() {
        let preset = preset_of(vec![("a", question(QuestionType::Noul, vec![]))]);
        assert!(validate(&preset).is_ok());
    }

    #[test]
    fn validate_rejects_criteria_for_unknown_option() {
        let q = question_with_criteria(QuestionType::Choice, vec!["x"], &[("y", "nope")]);
        let preset = preset_of(vec![("a", q)]);
        assert!(validate(&preset).is_err());
    }

    #[test]
    fn build_request_sends_model_state_and_questions() {
        let preset = preset_of(vec![
            (
                "department",
                question(QuestionType::Choice, vec!["billing", "technical"]),
            ),
            ("needs_human", question(QuestionType::Noul, vec![])),
        ]);

        let req = build_request("laya", "I was billed twice", &preset, &BTreeMap::new()).unwrap();
        assert_eq!(req["model"], "laya");
        assert_eq!(req["state"], "I was billed twice");

        let dept = &req["questions"]["department"];
        assert_eq!(dept["type"], "choice");
        assert_eq!(dept["instructions"], "Pick one.");
        // criteria is an object: option -> description
        assert_eq!(dept["criteria"]["billing"], "billing");
        assert_eq!(dept["criteria"]["technical"], "technical");
        assert!(req["questions"]["needs_human"].get("criteria").is_none());
    }

    #[test]
    fn build_request_uses_option_descriptions_when_present() {
        let preset = preset_of(vec![(
            "department",
            question_with_criteria(
                QuestionType::Choice,
                vec!["billing", "technical"],
                &[("billing", "billing issues")],
            ),
        )]);

        let req = build_request("laya", "I was billed twice", &preset, &BTreeMap::new()).unwrap();
        let dept = &req["questions"]["department"];
        // A described option uses its description...
        assert_eq!(dept["criteria"]["billing"], "billing issues");
        // ...an undescribed option falls back to its own name.
        assert_eq!(dept["criteria"]["technical"], "technical");
    }

    #[test]
    fn read_state_prefers_text_and_trims() {
        assert_eq!(
            read_state(&Some("  hello ".to_string()), &None).unwrap(),
            "hello"
        );
    }

    #[test]
    fn read_state_reads_file() {
        let path = std::env::temp_dir().join(format!("rone-test-{}.txt", std::process::id()));
        std::fs::write(&path, "from file\n").unwrap();
        assert_eq!(read_state(&None, &Some(path.clone())).unwrap(), "from file");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_state_rejects_empty_input() {
        assert!(read_state(&Some("   ".to_string()), &None).is_err());
    }

    #[test]
    fn extract_answers_pulls_answers_map() {
        let response = json!({"answers": {"a": {"type": "noul", "noul": 0.5}}});
        let answers = extract_answers(&response);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers["a"]["noul"], 0.5);
    }

    #[test]
    fn extract_answers_empty_when_missing() {
        assert!(extract_answers(&json!({"ok": true})).is_empty());
    }

    #[test]
    fn render_answers_marks_chosen_option_sorted_by_probability() {
        let response = json!({
            "answers": {
                "department": {
                    "type": "choice",
                    "choice": "billing",
                    "probabilities": {"billing": 0.97, "technical": 0.02, "sales": 0.01}
                }
            }
        });
        let answers = extract_answers(&response);
        let out = render_answers(&answers, false);
        assert!(out.contains("*billing"));
        assert!(out.contains("97.00%"));
        assert!(out.find("billing").unwrap() < out.find("technical").unwrap());
    }

    #[test]
    fn render_answers_shows_noul_probability() {
        let response = json!({"answers": {"needs_human": {"type": "noul", "noul": 0.344}}});
        let answers = extract_answers(&response);
        let out = render_answers(&answers, false);
        assert!(out.contains("yes:"));
        assert!(out.contains("34.40%"));
    }

    #[test]
    fn render_answers_json_mode_is_valid_json() {
        let response = json!({"answers": {"a": {"type": "noul", "noul": 0.5}}});
        let answers = extract_answers(&response);
        let out = render_answers(&answers, true);
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["a"]["noul"], 0.5);
    }

    #[test]
    fn answer_confidence_prefers_confidence_field() {
        let answer = json!({"choice": "billing", "confidence": 0.95, "noul": 0.1});
        assert_eq!(answer_confidence(&answer), Some(0.95));
    }

    #[test]
    fn answer_confidence_falls_back_to_noul() {
        let answer = json!({"noul": 0.34});
        assert_eq!(answer_confidence(&answer), Some(0.34));
    }

    #[test]
    fn answer_confidence_falls_back_to_prob() {
        let answer = json!({"prob": 0.7});
        assert_eq!(answer_confidence(&answer), Some(0.7));
    }

    #[test]
    fn answer_confidence_none_when_absent() {
        let answer = json!({"type": "choice"});
        assert_eq!(answer_confidence(&answer), None);
    }

    #[test]
    fn below_threshold_flags_low_and_missing_confidence() {
        let response = json!({
            "answers": {
                "high": {"choice": "a", "confidence": 0.95},
                "low": {"choice": "b", "confidence": 0.5},
                "noconf": {"type": "choice"}
            }
        });
        let answers = extract_answers(&response);
        let flagged = below_threshold(&answers, 0.9);
        let ids: Vec<&str> = flagged.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["low", "noconf"]);
    }

    #[test]
    fn below_threshold_accepts_all_above() {
        let response = json!({
            "answers": {
                "a": {"noul": 0.95},
                "b": {"prob": 1.0}
            }
        });
        let answers = extract_answers(&response);
        assert!(below_threshold(&answers, 0.9).is_empty());
    }

    #[test]
    fn parse_vars_parses_name_value_pairs() {
        let vars =
            parse_vars(&["dept=billing".to_string(), "note=two=equals".to_string()]).unwrap();
        assert_eq!(vars["dept"], "billing");
        // The split happens at the first `=`; the value may contain `=`.
        assert_eq!(vars["note"], "two=equals");
    }

    #[test]
    fn parse_vars_rejects_missing_equals() {
        assert!(parse_vars(&["billing".to_string()]).is_err());
    }

    #[test]
    fn parse_vars_rejects_empty_name() {
        assert!(parse_vars(&["=billing".to_string()]).is_err());
    }

    #[test]
    fn substitute_replaces_known_placeholders() {
        let vars = parse_vars(&["dept=billing".to_string()]).unwrap();
        assert_eq!(
            substitute("Route this to {{dept}}.", &vars).unwrap(),
            "Route this to billing."
        );
        // Text without placeholders is left untouched.
        assert_eq!(substitute("Plain text.", &vars).unwrap(), "Plain text.");
    }

    #[test]
    fn substitute_rejects_unknown_placeholder() {
        let vars = parse_vars(&["dept=billing".to_string()]).unwrap();
        assert!(substitute("Route to {{nope}}.", &vars).is_err());
    }

    #[test]
    fn build_request_substitutes_variables_in_instructions() {
        let preset = Preset {
            questions: BTreeMap::from([(
                "department".to_string(),
                QuestionDef {
                    kind: QuestionType::Choice,
                    instructions: "Route this to {{dept}}.".to_string(),
                    options: vec!["billing".to_string(), "technical".to_string()],
                    criteria: BTreeMap::new(),
                },
            )]),
        };
        let vars = parse_vars(&["dept=billing".to_string()]).unwrap();

        let req = build_request("laya", "I was billed twice", &preset, &vars).unwrap();
        assert_eq!(
            req["questions"]["department"]["instructions"],
            "Route this to billing."
        );
    }
}
