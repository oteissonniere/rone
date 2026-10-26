//! Deterministic mock answers for `/v1/systemone`.
//!
//! The `rone-mock` server (`src/bin/mock_server.rs`) serves these in place of
//! a real System One model, so the CLI can be exercised end to end without an
//! inference server. Generation is a pure function of the input: the same
//! state and questions always produce the same answers, which keeps tests
//! stable and lets the CLI be debugged against known values.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::{QuestionDef, QuestionType};

/// FNV-1a 64-bit. Unlike `std::hash::DefaultHasher`, it is not randomized
/// per process: the same bytes always give the same value.
fn fnv1a(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Generate the `answers` object of a `/v1/systemone` response.
///
/// `state` is the input text, `questions` the preset questions. Each question
/// gets an answer shaped like what a real model returns:
/// - `choice` → `{type, choice, confidence, probabilities}`
/// - `noul`   → `{type, noul}`
/// - `score`  → `{type, score}` over a default 1..5 scale
pub fn generate_answers(state: &str, questions: &BTreeMap<String, QuestionDef>) -> Value {
    let mut answers = serde_json::Map::new();
    for (id, q) in questions {
        let seed = fnv1a(format!("{state}\u{0}{id}").as_bytes());
        let answer = match q.kind {
            QuestionType::Choice => choice_answer(q, seed),
            QuestionType::Noul => noul_answer(seed),
            QuestionType::Score => score_answer(seed),
        };
        answers.insert(id.clone(), answer);
    }
    Value::Object(answers)
}

/// The picked option is `seed % n`; it carries a confidence in 0.85..0.99 and
/// the remaining probability mass is split evenly over the other options.
fn choice_answer(q: &QuestionDef, seed: u64) -> Value {
    if q.options.is_empty() {
        return json!({ "type": "choice" });
    }
    let n = q.options.len();
    let winner = q.options[(seed % n as u64) as usize].clone();
    let confidence = if n == 1 {
        1.0
    } else {
        // 0.85..0.99: clears a 0.9 threshold most of the time, but not
        // always, so the below-threshold path is exercised too.
        0.85 + f64::from(((seed >> 16) % 150) as u32) / 1000.0
    };
    let mut probabilities = serde_json::Map::new();
    for opt in &q.options {
        let p = if *opt == winner {
            confidence
        } else {
            (1.0 - confidence) / f64::from((n - 1) as u32)
        };
        probabilities.insert(opt.clone(), json!(p));
    }
    json!({
        "type": "choice",
        "choice": winner,
        "confidence": confidence,
        "probabilities": probabilities
    })
}

/// A probability in 0.10..0.899: spans the 0.5 boundary and the usual
/// thresholds, so both outcomes are reachable.
fn noul_answer(seed: u64) -> Value {
    let p = 0.10 + f64::from((seed % 800) as u32) / 1000.0;
    json!({
        "type": "noul",
        "noul": p
    })
}

/// An integer score over the default 1..5 scale; the request carries no scale
/// for `score` questions, so the mock assumes one.
fn score_answer(seed: u64) -> Value {
    let score = 1 + (seed % 5) as u32;
    json!({
        "type": "score",
        "score": score
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{answer_confidence, extract_answers};

    fn choice_question(options: &[&str]) -> QuestionDef {
        QuestionDef {
            kind: QuestionType::Choice,
            instructions: "Pick one.".to_string(),
            options: options.iter().copied().map(String::from).collect(),
            criteria: BTreeMap::new(),
        }
    }

    fn noul_question() -> QuestionDef {
        QuestionDef {
            kind: QuestionType::Noul,
            instructions: "A human must handle this.".to_string(),
            options: vec![],
            criteria: BTreeMap::new(),
        }
    }

    fn score_question() -> QuestionDef {
        QuestionDef {
            kind: QuestionType::Score,
            instructions: "Rate the severity.".to_string(),
            options: vec![],
            criteria: BTreeMap::new(),
        }
    }

    fn triage_questions() -> BTreeMap<String, QuestionDef> {
        BTreeMap::from([
            (
                "department".to_string(),
                choice_question(&["billing", "technical", "sales"]),
            ),
            ("needs_human".to_string(), noul_question()),
            ("severity".to_string(), score_question()),
        ])
    }

    const STATE: &str = "I was billed twice for the same invoice.";

    #[test]
    fn fnv1a_is_stable_and_sensitive() {
        assert_eq!(fnv1a(b"hello"), fnv1a(b"hello"));
        assert_ne!(fnv1a(b"hello"), fnv1a(b"world"));
    }

    #[test]
    fn same_input_gives_same_answers() {
        let questions = triage_questions();
        assert_eq!(
            generate_answers(STATE, &questions),
            generate_answers(STATE, &questions)
        );
    }

    #[test]
    fn answers_cover_every_question() {
        let questions = triage_questions();
        let answers = generate_answers(STATE, &questions);
        let keys: Vec<&str> = answers
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["department", "needs_human", "severity"]);
    }

    #[test]
    fn choice_answer_shape_and_probability_mass() {
        let questions = BTreeMap::from([("q".to_string(), choice_question(&["a", "b", "c"]))]);
        let answer = generate_answers("some state", &questions)["q"].clone();
        assert_eq!(answer["type"], "choice");
        let choice = answer["choice"].as_str().unwrap();
        assert!(["a", "b", "c"].contains(&choice));
        let confidence = answer["confidence"].as_f64().unwrap();
        assert!((0.85..1.0).contains(&confidence));
        let probs = answer["probabilities"].as_object().unwrap();
        assert_eq!(probs[choice].as_f64().unwrap(), confidence);
        let sum: f64 = probs.values().filter_map(Value::as_f64).sum();
        assert!((sum - 1.0).abs() < 1e-9);
    }

    #[test]
    fn choice_with_single_option_takes_it() {
        let questions = BTreeMap::from([("q".to_string(), choice_question(&["only"]))]);
        let answer = generate_answers("state", &questions)["q"].clone();
        assert_eq!(answer["choice"], "only");
        assert_eq!(answer["confidence"], 1.0);
        assert_eq!(answer["probabilities"]["only"], 1.0);
    }

    #[test]
    fn noul_answer_stays_in_range() {
        let questions = BTreeMap::from([("q".to_string(), noul_question())]);
        for i in 0..64 {
            let answer = generate_answers(&format!("state {i}"), &questions)["q"].clone();
            assert_eq!(answer["type"], "noul");
            let p = answer["noul"].as_f64().unwrap();
            assert!((0.10..0.90).contains(&p), "p = {p}");
        }
    }

    #[test]
    fn score_answer_stays_on_the_default_scale() {
        let questions = BTreeMap::from([("q".to_string(), score_question())]);
        for i in 0..32 {
            let answer = generate_answers(&format!("state {i}"), &questions)["q"].clone();
            assert_eq!(answer["type"], "score");
            let score = answer["score"].as_u64().unwrap();
            assert!((1..=5).contains(&score), "score = {score}");
        }
    }

    #[test]
    fn mock_answers_flow_through_extract_and_confidence() {
        let questions = triage_questions();
        let response = json!({
            "model": "mock",
            "state": STATE,
            "answers": generate_answers(STATE, &questions),
        });
        let answers = extract_answers(&response);
        assert_eq!(answers.len(), 3);
        assert!(answer_confidence(&answers["department"]).is_some());
        assert!(answer_confidence(&answers["needs_human"]).is_some());
        // A score answer carries no confidence, by design.
        assert!(answer_confidence(&answers["severity"]).is_none());
    }

    #[test]
    fn pinned_values_for_known_input() {
        // Pinned against the FNV-1a generation: any change to the seed mix or
        // the formulas is a behaviour change and must be reviewed.
        let answers = generate_answers(STATE, &triage_questions());
        let dept = &answers["department"];
        assert_eq!(dept["type"], "choice");
        assert_eq!(dept["choice"], "sales");
        assert!((dept["confidence"].as_f64().unwrap() - 0.977).abs() < 1e-12);
        let probs = dept["probabilities"].as_object().unwrap();
        assert!((probs["sales"].as_f64().unwrap() - 0.977).abs() < 1e-12);
        assert!((probs["billing"].as_f64().unwrap() - 0.0115).abs() < 1e-12);
        assert!((probs["technical"].as_f64().unwrap() - 0.0115).abs() < 1e-12);
        assert!((answers["needs_human"]["noul"].as_f64().unwrap() - 0.761).abs() < 1e-12);
        assert_eq!(answers["severity"]["score"], 5);
    }
}
