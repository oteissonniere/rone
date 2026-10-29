//! Rone library: build and interpret `/v1/systemone` requests.
//!
//! No HTTP, no CLI: just presets, validation, request building, answer
//! extraction and rendering. The `rone` binary (`src/main.rs`) adds the
//! command line and the network call on top.

/// `GET /v1/models` response envelope (OpenAI-compatible, e.g. LM Studio).
#[derive(Deserialize, Debug, PartialEq)]
pub struct ModelList {
    #[serde(default)]
    pub data: Vec<ModelInfo>,
}

/// One model entry inside `GET /v1/models`.
#[derive(Deserialize, Debug, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use color::ColorMode;

pub mod color;
pub mod config;
pub mod mock;

/// A preset file: `presets/<name>.toml`.
#[derive(Deserialize, Debug, PartialEq)]
pub struct Preset {
    pub questions: BTreeMap<String, QuestionDef>,
}

#[derive(Deserialize, Debug, PartialEq)]
pub struct QuestionDef {
    #[serde(rename = "type")]
    pub kind: QuestionType,
    pub instructions: String,
    #[serde(default)]
    pub options: Vec<String>,
    /// Optional option → description, sent as `criteria` for `choice`
    /// questions. Options without a description use their own name.
    #[serde(default)]
    pub criteria: BTreeMap<String, String>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum QuestionType {
    Choice,
    Score,
    Noul,
}

impl QuestionType {
    pub fn as_str(self) -> &'static str {
        match self {
            QuestionType::Choice => "choice",
            QuestionType::Score => "score",
            QuestionType::Noul => "noul",
        }
    }
}

/// Load a preset from `presets/<name>.toml`.
pub fn load_preset(name: &str) -> Result<Preset> {
    let path = PathBuf::from("presets").join(format!("{name}.toml"));
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read preset file {}", path.display()))?;
    let preset =
        parse_preset(&content).with_context(|| format!("in preset file {}", path.display()))?;
    Ok(preset)
}

/// Load several presets and merge their question sets into one preset:
/// every question of every given preset, answered in a single request. A
/// question id defined in several presets is accepted only when the
/// definitions are identical (the same question); conflicting
/// definitions are an error.
pub fn load_presets(names: &[String]) -> Result<Preset> {
    if names.is_empty() {
        bail!("no preset given");
    }
    let mut merged = Preset {
        questions: BTreeMap::new(),
    };
    for name in names {
        let preset = load_preset(name)?;
        for (id, q) in preset.questions {
            match merged.questions.get(&id) {
                Some(existing) => {
                    if *existing != q {
                        bail!(
                            "question '{id}' is defined differently in preset '{name}'; \
                             keep a single definition for this question"
                        );
                    }
                }
                None => {
                    merged.questions.insert(id, q);
                }
            }
        }
    }
    Ok(merged)
}

/// List the names of the presets available in `presets/` (sorted).
pub fn list_presets() -> Result<Vec<String>> {
    let mut names = Vec::new();
    let entries =
        std::fs::read_dir("presets").with_context(|| "failed to read the presets/ directory")?;
    for entry in entries {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|os| anyhow::anyhow!("preset file name is not valid UTF-8: {os:?}"))?;
        if let Some(stem) = name.strip_suffix(".toml") {
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// One labeled evaluation case: an input text and the expected answer for
/// each question — a choice option name, `"yes"`/`"no"` for `noul`, a
/// number for `score`.
#[derive(Deserialize, Debug, PartialEq)]
pub struct EvalCase {
    pub input: String,
    #[serde(default)]
    pub expected: BTreeMap<String, Value>,
}

/// Parse an eval dataset: a JSON array of `{input, expected}` objects.
pub fn parse_eval_cases(content: &str) -> Result<Vec<EvalCase>> {
    let cases: Vec<EvalCase> = serde_json::from_str(content).context(
        "failed to parse the eval dataset: expected a JSON array of {input, expected} objects",
    )?;
    if cases.is_empty() {
        bail!("the eval dataset contains no cases");
    }
    Ok(cases)
}

/// Load an eval dataset from a file.
pub fn load_eval_cases(path: &Path) -> Result<Vec<EvalCase>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read eval dataset {}", path.display()))?;
    parse_eval_cases(&content).with_context(|| format!("in eval dataset {}", path.display()))
}

/// Render a list of preset names as a human-friendly listing, with a header
/// hinting at the `--preset` flag.
pub fn render_presets(names: &[String], colors: ColorMode) -> String {
    let mut out = String::from("Presets in presets/ (use --preset <name>):\n");
    for name in names {
        out.push_str(&format!("  {}\n", colors.bold(name)));
    }
    out
}

/// Parse a preset from TOML content.
pub fn parse_preset(content: &str) -> Result<Preset> {
    let preset: Preset = toml::from_str(content).context("failed to parse preset TOML")?;
    if preset.questions.is_empty() {
        bail!("preset defines no questions");
    }
    Ok(preset)
}

/// Catch common mistakes before sending, with clear messages.
pub fn validate(preset: &Preset) -> Result<()> {
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
pub fn read_state(text: &Option<String>, file: &Option<PathBuf>) -> Result<String> {
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

/// One state per line from --file or stdin; blank lines are skipped.
pub fn read_states(file: &Option<PathBuf>) -> Result<Vec<String>> {
    let raw = if let Some(f) = file {
        std::fs::read_to_string(f)
            .with_context(|| format!("failed to read file {}", f.display()))?
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read stdin")?;
        buf
    };
    let states: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect();
    if states.is_empty() {
        bail!("empty input: provide one state per line with --file or stdin");
    }
    Ok(states)
}

/// Parse `NAME=VALUE` pairs into an ordered map. The value may contain
/// further `=` characters; the split happens at the first one.
pub fn parse_vars(vars: &[String]) -> Result<BTreeMap<String, String>> {
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
pub fn substitute(text: &str, vars: &BTreeMap<String, String>) -> Result<String> {
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
pub fn build_request(
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

/// Pull the `answers` map out of a `/v1/systemone` response.
pub fn extract_answers(response: &Value) -> BTreeMap<String, Value> {
    response
        .get("answers")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// The confidence an answer carries: its `confidence` field for `choice`
/// questions, falling back to `noul` and `prob`.
pub fn answer_confidence(answer: &Value) -> Option<f64> {
    answer
        .get("confidence")
        .or_else(|| answer.get("noul"))
        .or_else(|| answer.get("prob"))
        .and_then(Value::as_f64)
}

/// The answers whose confidence is below `min`, or missing (in preset order).
pub fn below_threshold(answers: &BTreeMap<String, Value>, min: f64) -> Vec<(String, Option<f64>)> {
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

/// The value an answer carries as a comparable string: the picked option
/// for `choice`, `"yes"`/`"no"` for `noul` (0.5 and up is "yes"), the
/// score for `score`.
pub fn answer_value(answer: &Value) -> Option<String> {
    if let Some(choice) = answer.get("choice").and_then(Value::as_str) {
        return Some(choice.to_string());
    }
    if let Some(p) = answer.get("noul").and_then(Value::as_f64) {
        return Some(if p >= 0.5 { "yes".into() } else { "no".into() });
    }
    answer
        .get("score")
        .and_then(Value::as_u64)
        .map(|s| s.to_string())
}

/// Whether the model's answer matches the expected value: an option name,
/// `"yes"`/`"no"` for `noul`, or a number for `score`.
pub fn answer_matches(answer: &Value, expected: &Value) -> bool {
    match (answer_value(answer), expected_value(expected)) {
        (Some(got), Some(want)) => got == want,
        _ => false,
    }
}

/// The expected value as a comparable string: a string as-is, a number as
/// its decimal representation.
fn expected_value(expected: &Value) -> Option<String> {
    expected
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| expected.as_u64().map(|n| n.to_string()))
}

/// Render one set of answers as a readable table (optionally colored) or
/// as JSON.
pub fn render_answers(
    answers: &BTreeMap<String, Value>,
    as_json: bool,
    colors: ColorMode,
) -> String {
    if as_json {
        return serde_json::to_string_pretty(answers).unwrap_or_default();
    }
    let mut out = String::new();
    for (id, answer) in answers {
        out.push_str(&format!("{}:\n", colors.bold(id)));
        // choice: `probabilities` map + `choice` for the picked option
        if let Some(probs) = answer.get("probabilities").and_then(Value::as_object) {
            let mut entries: Vec<(String, f64)> = probs
                .iter()
                .filter_map(|(k, v)| v.as_f64().map(|p| (k.clone(), p)))
                .collect();
            entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let chosen = answer.get("choice").and_then(Value::as_str);
            for (name, prob) in &entries {
                let line = format!("{name:<24} {:>7.2}%", prob * 100.0);
                if chosen == Some(name.as_str()) {
                    // The picked option stands out in green.
                    out.push_str(&format!("  {}\n", colors.green(&format!("*{line}"))));
                } else {
                    out.push_str(&format!("  {line}\n"));
                }
            }
        } else if let Some(prob) = answer.get("noul").and_then(Value::as_f64) {
            let value = format!("{:>8.2}%", prob * 100.0);
            out.push_str(&format!("  yes: {}\n", colors.magnitude(&value, prob)));
        } else if let Some(score) = answer.get("score").and_then(Value::as_u64) {
            // The default scale is 1..5, so normalize against 5.
            let value = score.to_string();
            out.push_str(&format!(
                "  score: {}\n",
                colors.magnitude(&value, score as f64 / 5.0)
            ));
        } else if let Some(prob) = answer.get("prob").and_then(Value::as_f64) {
            let value = format!("{:>8.2}%", prob * 100.0);
            out.push_str(&format!("  {}\n", colors.magnitude(&value, prob)));
        } else {
            out.push_str(&format!("  {answer}\n"));
        }
    }
    out
}

/// Format a duration as a short human-friendly string: `842 ms` below one
/// second, `1.2 s` at a second and above.
pub fn format_duration(elapsed: Duration) -> String {
    let ms = elapsed.as_millis();
    if ms >= 1000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{ms} ms")
    }
}

/// Render a request latency as a display line: `elapsed: 842 ms`.
pub fn render_latency(elapsed: Duration) -> String {
    format!("elapsed: {}", format_duration(elapsed))
}

/// Render the list of available models: one line per model, the display name
/// (falling back to the model id) in bold, followed by the description when
/// present.
pub fn render_models(models: &[ModelInfo], colors: ColorMode) -> String {
    let mut out = String::new();
    for model in models {
        let name = colors.bold(model.name.as_deref().unwrap_or(&model.id));
        match model.description.as_deref() {
            Some(description) => out.push_str(&format!("{name}: {description}")),
            None => out.push_str(&name),
        }
        out.push('\n');
    }
    out
}

/// Render a batch of (input, answers, latency) triples: one numbered block
/// per input in text mode, each followed by its `elapsed: ...` line, and a
/// JSON array of `{"input", "answers"}` objects in JSON mode.
pub fn render_batch(
    results: &[(String, BTreeMap<String, Value>, Duration)],
    as_json: bool,
    colors: ColorMode,
) -> String {
    if as_json {
        let arr: Vec<Value> = results
            .iter()
            .map(|(input, answers, _)| json!({ "input": input, "answers": answers }))
            .collect();
        return serde_json::to_string_pretty(&arr).unwrap_or_default();
    }
    let mut out = String::new();
    for (i, (input, answers, elapsed)) in results.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!(
            "{}\n",
            colors.bold(&format!("#{}: {input}", i + 1))
        ));
        out.push_str(&render_answers(answers, false, colors));
        out.push('\n');
        out.push_str(&format!("{}\n", render_latency(*elapsed)));
    }
    out
}

/// Render comparison results: the same questions answered by several
/// profiles, one labeled block per profile in text mode, each followed by
/// its `elapsed: ...` line, and a JSON array of
/// `{"profile", "model", "answers"}` objects in JSON mode.
pub fn render_comparison(
    results: &[(String, String, BTreeMap<String, Value>, Duration)],
    as_json: bool,
    colors: ColorMode,
) -> String {
    if as_json {
        let arr: Vec<Value> = results
            .iter()
            .map(|(profile, model, answers, _)| {
                json!({ "profile": profile, "model": model, "answers": answers })
            })
            .collect();
        return serde_json::to_string_pretty(&arr).unwrap_or_default();
    }
    let mut out = String::new();
    for (i, (profile, model, answers, elapsed)) in results.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!(
            "{}\n",
            colors.bold(&format!("--- {profile} ({model}) ---"))
        ));
        out.push_str(&render_answers(answers, false, colors));
        out.push('\n');
        out.push_str(&format!("{}\n", render_latency(*elapsed)));
    }
    out
}

/// The outcome of one eval case: the input, the expected answers, and the
/// answers the model actually produced.
pub struct EvalResult {
    pub input: String,
    pub expected: BTreeMap<String, Value>,
    pub answers: BTreeMap<String, Value>,
}

/// One miss in an eval report: the case index (1-based), its input, the
/// question, and the expected vs. got values (`<missing>` when the model
/// answered nothing).
struct EvalMiss {
    index: usize,
    input: String,
    question: String,
    expected: String,
    got: String,
}

/// Render the eval report: per-question accuracy over all cases plus the
/// list of misses, or a JSON object with the same tallies.
pub fn render_eval_report(results: &[EvalResult], as_json: bool, colors: ColorMode) -> String {
    let (questions, misses) = tally_eval(results);
    let total: usize = questions.iter().map(|(_, _, t)| *t).sum();
    let correct: usize = questions.iter().map(|(_, c, _)| *c).sum();
    if as_json {
        let questions: Vec<Value> = questions
            .iter()
            .map(|(q, c, t)| {
                json!({
                    "question": q,
                    "correct": c,
                    "total": t,
                    "accuracy": accuracy_ratio(*c, *t),
                })
            })
            .collect();
        let misses: Vec<Value> = misses
            .iter()
            .map(|m| {
                json!({
                    "input": m.input,
                    "question": m.question,
                    "expected": m.expected,
                    "got": m.got,
                })
            })
            .collect();
        return serde_json::to_string_pretty(&json!({
            "inputs": results.len(),
            "answers": total,
            "questions": questions,
            "misses": misses,
            "overall": {
                "correct": correct,
                "total": total,
                "accuracy": accuracy_ratio(correct, total),
            },
        }))
        .unwrap_or_default();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "{}\n",
        colors.bold(&format!(
            "eval: {} inputs, {} answers",
            results.len(),
            total
        ))
    ));
    for (q, c, t) in &questions {
        let ratio = accuracy_ratio(*c, *t);
        let pct = format!("{:.1}%", ratio * 100.0);
        out.push_str(&format!(
            "  {:12}  {}/{}  {}\n",
            q,
            c,
            t,
            paint_accuracy(colors, &pct, ratio)
        ));
    }
    if misses.is_empty() {
        out.push_str("all answers match\n");
        return out;
    }
    out.push_str(&colors.bold("misses:\n"));
    let mut last_index: Option<usize> = None;
    for miss in &misses {
        if last_index != Some(miss.index) {
            out.push_str(&format!("  [{}] {:?}\n", miss.index, miss.input));
            last_index = Some(miss.index);
        }
        out.push_str(&format!(
            "      {}: expected {}, got {}\n",
            miss.question,
            miss.expected,
            colors.red(&miss.got)
        ));
    }
    out
}

/// Tally the eval results: per-question `(correct, total)` in question
/// order, plus the misses in case order.
fn tally_eval(results: &[EvalResult]) -> (Vec<(String, usize, usize)>, Vec<EvalMiss>) {
    let mut per_question: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut misses = Vec::new();
    for (i, r) in results.iter().enumerate() {
        for (q, want) in &r.expected {
            let entry = per_question.entry(q.clone()).or_insert((0, 0));
            entry.1 += 1;
            let answer = r.answers.get(q).unwrap_or(&Value::Null);
            if answer_matches(answer, want) {
                entry.0 += 1;
            } else {
                misses.push(EvalMiss {
                    index: i + 1,
                    input: r.input.clone(),
                    question: q.clone(),
                    expected: expected_value(want).unwrap_or_default(),
                    got: answer_value(answer).unwrap_or_else(|| "<missing>".to_string()),
                });
            }
        }
    }
    let questions = per_question
        .into_iter()
        .map(|(q, (c, t))| (q, c, t))
        .collect();
    (questions, misses)
}

/// The accuracy ratio of `correct` out of `total` (0 when `total` is 0).
fn accuracy_ratio(correct: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        correct as f64 / total as f64
    }
}

/// Paint an accuracy percentage: green at 90% and up, yellow from 70%,
/// red below.
fn paint_accuracy(colors: ColorMode, text: &str, ratio: f64) -> String {
    if ratio >= 0.9 {
        colors.green(text)
    } else if ratio >= 0.7 {
        colors.yellow(text)
    } else {
        colors.red(text)
    }
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
    fn list_presets_finds_the_repo_presets() {
        let names = list_presets().unwrap();
        assert!(names.contains(&"triage".to_string()));
    }

    #[test]
    fn load_presets_merges_question_sets() {
        // `needs_human` is defined identically in triage and severity, so
        // it is deduplicated, not an error.
        let preset = load_presets(&["triage".to_string(), "severity".to_string()]).unwrap();
        let ids: Vec<&str> = preset.questions.keys().map(String::as_str).collect();
        assert_eq!(ids, vec!["department", "needs_human", "severity"]);
    }

    #[test]
    fn load_presets_single_name_is_the_preset_itself() {
        let merged = load_presets(&["triage".to_string()]).unwrap();
        let single = load_preset("triage").unwrap();
        assert_eq!(merged, single);
    }

    #[test]
    fn load_presets_rejects_conflicting_definitions() {
        // escalation defines needs_human with different instructions.
        let err = load_presets(&["triage".to_string(), "escalation".to_string()]).unwrap_err();
        assert!(err.to_string().contains("needs_human"));
        assert!(err.to_string().contains("escalation"));
    }

    #[test]
    fn load_presets_rejects_empty_list() {
        assert!(load_presets(&[]).is_err());
    }

    #[test]
    fn render_presets_lists_names_under_a_header() {
        let names = vec!["triage".to_string(), "severity".to_string()];
        let out = render_presets(&names, ColorMode::off());
        assert_eq!(
            out,
            "Presets in presets/ (use --preset <name>):\n  triage\n  severity\n"
        );
    }

    #[test]
    fn render_presets_empty_list_is_header_only() {
        let out = render_presets(&[], ColorMode::off());
        assert_eq!(out, "Presets in presets/ (use --preset <name>):\n");
    }

    #[test]
    fn render_presets_colored_mode_paints_ansi() {
        let names = vec!["triage".to_string()];
        assert!(render_presets(&names, ColorMode::on()).contains('\u{1b}'));
        assert!(!render_presets(&names, ColorMode::off()).contains('\u{1b}'));
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
        let out = render_answers(&answers, false, ColorMode::off());
        assert!(out.contains("*billing"));
        assert!(out.contains("97.00%"));
        assert!(out.find("billing").unwrap() < out.find("technical").unwrap());
    }

    #[test]
    fn render_answers_shows_noul_probability() {
        let response = json!({"answers": {"needs_human": {"type": "noul", "noul": 0.344}}});
        let answers = extract_answers(&response);
        let out = render_answers(&answers, false, ColorMode::off());
        assert!(out.contains("yes:"));
        assert!(out.contains("34.40%"));
    }

    #[test]
    fn render_answers_shows_score_value() {
        let response = json!({"answers": {"severity": {"type": "score", "score": 4}}});
        let answers = extract_answers(&response);
        let out = render_answers(&answers, false, ColorMode::off());
        assert!(out.contains("severity:"));
        assert!(out.contains("score: 4"));
        // No raw JSON fallback.
        assert!(!out.contains('{'));
    }

    #[test]
    fn render_answers_colored_mode_paints_ansi() {
        let response = json!({
            "answers": {
                "department": {
                    "type": "choice",
                    "choice": "billing",
                    "probabilities": {"billing": 0.97, "technical": 0.03}
                },
                "needs_human": {"type": "noul", "noul": 0.344},
                "severity": {"type": "score", "score": 4}
            }
        });
        let answers = extract_answers(&response);
        let out = render_answers(&answers, false, ColorMode::on());
        // The question id is bold.
        assert!(out.contains("[1mdepartment"));
        // The chosen option is green.
        assert!(out.contains("[32m*billing"));
        // A low noul (< 0.5) is green.
        assert!(out.contains("[32m   34.40%"));
        // A score of 4 out of 5 (0.8) is red.
        assert!(out.contains("[31m4"));
    }

    #[test]
    fn render_answers_off_mode_has_no_ansi_escapes() {
        let response = json!({"answers": {"a": {"type": "noul", "noul": 0.5}}});
        let answers = extract_answers(&response);
        let out = render_answers(&answers, false, ColorMode::off());
        assert!(!out.contains('\u{1b}'));
    }

    #[test]
    fn render_answers_json_mode_is_valid_json() {
        let response = json!({"answers": {"a": {"type": "noul", "noul": 0.5}}});
        let answers = extract_answers(&response);
        let out = render_answers(&answers, true, ColorMode::off());
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

    #[test]
    fn read_states_splits_lines_and_skips_blank() {
        let path = std::env::temp_dir().join(format!("rone-batch-{}.txt", std::process::id()));
        std::fs::write(&path, "first ticket\n\n  second ticket  \n\n").unwrap();
        let states = read_states(&Some(path.clone())).unwrap();
        assert_eq!(states, vec!["first ticket", "second ticket"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_states_rejects_empty_input() {
        let path =
            std::env::temp_dir().join(format!("rone-batch-empty-{}.txt", std::process::id()));
        std::fs::write(&path, "   \n\n").unwrap();
        assert!(read_states(&Some(path.clone())).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn render_batch_text_has_numbered_blocks() {
        let answers = extract_answers(&json!({
            "answers": {
                "department": {
                    "type": "choice",
                    "choice": "billing",
                    "probabilities": {"billing": 0.97, "technical": 0.03}
                }
            }
        }));
        let results = vec![(
            "I was billed twice".to_string(),
            answers,
            Duration::from_millis(842),
        )];
        let out = render_batch(&results, false, ColorMode::off());
        assert!(out.starts_with("#1: I was billed twice\n"));
        assert!(out.contains("*billing"));
        assert!(out.contains("97.00%"));
        // The latency line sits under the answers.
        assert!(out.contains("elapsed: 842 ms"));
    }

    #[test]
    fn render_batch_json_is_array_of_input_answers() {
        let response = json!({"answers": {"a": {"noul": 0.5}}});
        let answers = extract_answers(&response);
        let results = vec![
            (
                "first".to_string(),
                answers.clone(),
                Duration::from_millis(11),
            ),
            ("second".to_string(), answers, Duration::from_millis(22)),
        ];
        let out = render_batch(&results, true, ColorMode::off());
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["input"], "first");
        assert_eq!(arr[1]["input"], "second");
        assert_eq!(arr[0]["answers"]["a"]["noul"], 0.5);
    }

    #[test]
    fn render_comparison_text_has_labeled_blocks() {
        let answers = extract_answers(&json!({
            "answers": {
                "department": {
                    "type": "choice",
                    "choice": "billing",
                    "probabilities": {"billing": 0.97, "technical": 0.03}
                }
            }
        }));
        let results = vec![
            (
                "macbookpro".to_string(),
                "laya".to_string(),
                answers.clone(),
                Duration::from_millis(842),
            ),
            (
                "mini-ai".to_string(),
                "laya".to_string(),
                answers,
                Duration::from_millis(900),
            ),
        ];
        let out = render_comparison(&results, false, ColorMode::off());
        assert!(out.contains("--- macbookpro (laya) ---"));
        assert!(out.contains("--- mini-ai (laya) ---"));
        // Each profile gets its own block of answers.
        assert_eq!(out.matches("*billing").count(), 2);
        // Each block carries its own latency line.
        assert_eq!(
            out.lines().filter(|l| l.starts_with("elapsed: ")).count(),
            2
        );
    }

    #[test]
    fn render_comparison_json_is_array_with_profile_and_model() {
        let answers = extract_answers(&json!({"answers": {"a": {"noul": 0.5}}}));
        let results = vec![
            (
                "p1".to_string(),
                "m1".to_string(),
                answers.clone(),
                Duration::from_millis(11),
            ),
            (
                "p2".to_string(),
                "m2".to_string(),
                answers,
                Duration::from_millis(22),
            ),
        ];
        let out = render_comparison(&results, true, ColorMode::off());
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["profile"], "p1");
        assert_eq!(arr[0]["model"], "m1");
        assert_eq!(arr[0]["answers"]["a"]["noul"], 0.5);
        assert_eq!(arr[1]["profile"], "p2");
    }

    #[test]
    fn format_duration_uses_ms_below_one_second() {
        assert_eq!(format_duration(Duration::from_millis(842)), "842 ms");
        assert_eq!(format_duration(Duration::from_millis(999)), "999 ms");
    }

    #[test]
    fn format_duration_uses_seconds_from_one_second_up() {
        assert_eq!(format_duration(Duration::from_millis(1000)), "1.0 s");
        assert_eq!(format_duration(Duration::from_millis(1234)), "1.2 s");
    }

    #[test]
    fn render_latency_is_a_display_line() {
        assert_eq!(
            render_latency(Duration::from_millis(842)),
            "elapsed: 842 ms"
        );
    }

    #[test]
    fn model_list_parses_openai_envelope() {
        let raw = r#"{"object":"list","data":[
            {"id":"mock","object":"model"},
            {"id":"laya","name":"Laya 8B","description":"A decision model"}
        ]}"#;
        let list: ModelList = serde_json::from_str(raw).unwrap();
        assert_eq!(list.data.len(), 2);
        assert_eq!(list.data[1].name.as_deref(), Some("Laya 8B"));
        assert_eq!(list.data[0].name, None);
    }

    #[test]
    fn model_list_empty_data_is_valid() {
        let list: ModelList = serde_json::from_str(r#"{"object":"list","data":[]}"#).unwrap();
        assert!(list.data.is_empty());
    }

    #[test]
    fn render_models_one_line_per_model() {
        let models = vec![
            ModelInfo {
                id: "mock".into(),
                name: None,
                description: None,
            },
            ModelInfo {
                id: "laya".into(),
                name: Some("Laya 8B".into()),
                description: Some("A decision model".into()),
            },
        ];
        assert_eq!(
            render_models(&models, ColorMode::off()),
            "mock\nLaya 8B: A decision model\n"
        );
        let colored = render_models(&models, ColorMode::on());
        assert!(colored.contains("A decision model\n"));
        assert_ne!(colored, render_models(&models, ColorMode::off()));
    }

    #[test]
    fn parse_eval_cases_reads_input_and_expected() {
        let content = r#"[
            { "input": "I was billed twice",
              "expected": { "department": "billing", "needs_human": "yes" } },
            { "input": "The login page is down",
              "expected": { "department": "technical", "severity": 4 } }
        ]"#;
        let cases = parse_eval_cases(content).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].input, "I was billed twice");
        assert_eq!(cases[0].expected["department"], "billing");
        assert_eq!(cases[0].expected["needs_human"], "yes");
        assert_eq!(cases[1].expected["severity"], 4);
    }

    #[test]
    fn parse_eval_cases_empty_expected_is_fine() {
        let cases = parse_eval_cases(r#"[{ "input": "hello" }]"#).unwrap();
        assert!(cases[0].expected.is_empty());
    }

    #[test]
    fn parse_eval_cases_rejects_empty_array() {
        assert!(parse_eval_cases("[]").is_err());
    }

    #[test]
    fn parse_eval_cases_rejects_invalid_json() {
        assert!(parse_eval_cases("{ not json").is_err());
    }

    #[test]
    fn load_eval_cases_reads_a_file() {
        let path = std::env::temp_dir().join(format!("rone-eval-{}.json", std::process::id()));
        std::fs::write(&path, r#"[{ "input": "hello", "expected": { "a": "x" } }]"#).unwrap();
        let cases = load_eval_cases(&path).unwrap();
        assert_eq!(cases[0].input, "hello");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_eval_cases_reports_missing_file() {
        let path = std::env::temp_dir().join("rone-eval-missing.json");
        let err = load_eval_cases(&path).unwrap_err();
        assert!(err.to_string().contains("failed to read eval dataset"));
    }

    #[test]
    fn answer_value_choice_is_the_picked_option() {
        let answer = json!({"type": "choice", "choice": "billing", "confidence": 0.9});
        assert_eq!(answer_value(&answer), Some("billing".to_string()));
    }

    #[test]
    fn answer_value_noul_is_yes_no_at_half() {
        assert_eq!(
            answer_value(&json!({"type": "noul", "noul": 0.76})),
            Some("yes".to_string())
        );
        assert_eq!(
            answer_value(&json!({"type": "noul", "noul": 0.5})),
            Some("yes".to_string())
        );
        assert_eq!(
            answer_value(&json!({"type": "noul", "noul": 0.49})),
            Some("no".to_string())
        );
    }

    #[test]
    fn answer_value_score_is_the_number() {
        let answer = json!({"type": "score", "score": 4});
        assert_eq!(answer_value(&answer), Some("4".to_string()));
    }

    #[test]
    fn answer_value_none_when_no_known_field() {
        assert_eq!(answer_value(&json!({"type": "choice"})), None);
    }

    #[test]
    fn answer_matches_choice_and_noul() {
        assert!(answer_matches(
            &json!({"choice": "billing"}),
            &json!("billing")
        ));
        assert!(!answer_matches(
            &json!({"choice": "billing"}),
            &json!("sales")
        ));
        assert!(answer_matches(&json!({"noul": 0.8}), &json!("yes")));
        assert!(!answer_matches(&json!({"noul": 0.2}), &json!("yes")));
    }

    #[test]
    fn answer_matches_score_as_number() {
        assert!(answer_matches(&json!({"score": 4}), &json!(4)));
        assert!(!answer_matches(&json!({"score": 4}), &json!(3)));
    }

    #[test]
    fn answer_matches_false_when_answer_has_no_value() {
        assert!(!answer_matches(
            &json!({"type": "choice"}),
            &json!("billing")
        ));
    }

    /// One eval case with two matching answers and one with two misses.
    fn eval_results() -> Vec<EvalResult> {
        let case1 = EvalResult {
            input: "I was billed twice".to_string(),
            expected: serde_json::from_str(r#"{"department": "billing", "needs_human": "yes"}"#)
                .unwrap(),
            answers: serde_json::from_str(
                r#"{"department": {"choice": "billing"}, "needs_human": {"noul": 0.8}}"#,
            )
            .unwrap(),
        };
        let case2 = EvalResult {
            input: "My router keeps dropping".to_string(),
            expected: serde_json::from_str(r#"{"department": "technical", "severity": 4}"#)
                .unwrap(),
            answers: serde_json::from_str(
                r#"{"department": {"choice": "sales"}, "severity": {"score": 5}}"#,
            )
            .unwrap(),
        };
        vec![case1, case2]
    }

    #[test]
    fn render_eval_report_tallies_questions_and_misses() {
        let report = render_eval_report(&eval_results(), false, ColorMode::off());
        assert_eq!(
            report,
            "eval: 2 inputs, 4 answers\n  department    1/2  50.0%\n  needs_human   1/1  100.0%\n  severity      0/1  0.0%\nmisses:\n  [2] \"My router keeps dropping\"\n      department: expected technical, got sales\n      severity: expected 4, got 5\n"
        );
    }

    #[test]
    fn render_eval_report_says_all_match_when_no_miss() {
        let results = vec![EvalResult {
            input: "I was billed twice".to_string(),
            expected: serde_json::from_str(r#"{"department": "billing"}"#).unwrap(),
            answers: serde_json::from_str(r#"{"department": {"choice": "billing"}}"#).unwrap(),
        }];
        let report = render_eval_report(&results, false, ColorMode::off());
        assert_eq!(
            report,
            "eval: 1 inputs, 1 answers\n  department    1/1  100.0%\nall answers match\n"
        );
    }

    #[test]
    fn render_eval_report_marks_missing_answers() {
        let results = vec![EvalResult {
            input: "hello".to_string(),
            expected: serde_json::from_str(r#"{"a": "x"}"#).unwrap(),
            answers: BTreeMap::new(),
        }];
        let report = render_eval_report(&results, false, ColorMode::off());
        assert!(report.contains("a: expected x, got <missing>"));
    }

    #[test]
    fn render_eval_report_json_shape() {
        let report = render_eval_report(&eval_results(), true, ColorMode::off());
        let value: Value = serde_json::from_str(&report).unwrap();
        assert_eq!(value["inputs"], 2);
        assert_eq!(value["answers"], 4);
        assert_eq!(value["overall"]["correct"], 2);
        assert_eq!(value["overall"]["total"], 4);
        assert_eq!(value["overall"]["accuracy"], 0.5);
        let questions: Vec<&Value> = value["questions"].as_array().unwrap().iter().collect();
        assert_eq!(questions[0]["question"], "department");
        assert_eq!(questions[0]["accuracy"], 0.5);
        assert_eq!(questions[1]["question"], "needs_human");
        assert_eq!(questions[1]["accuracy"], 1.0);
        assert_eq!(value["misses"].as_array().unwrap().len(), 2);
        assert_eq!(value["misses"][0]["question"], "department");
        assert_eq!(value["misses"][0]["expected"], "technical");
        assert_eq!(value["misses"][0]["got"], "sales");
        assert_eq!(value["misses"][1]["question"], "severity");
        assert_eq!(value["misses"][1]["expected"], "4");
        assert_eq!(value["misses"][1]["got"], "5");
    }

    #[test]
    fn render_eval_report_colors_the_accuracy() {
        let report = render_eval_report(&eval_results(), false, ColorMode::on());
        assert!(report.contains("\u{1b}[31m50.0%\u{1b}[0m"));
        assert!(report.contains("\u{1b}[32m100.0%\u{1b}[0m"));
        assert!(report.contains("\u{1b}[1mmisses:"));
    }
}
