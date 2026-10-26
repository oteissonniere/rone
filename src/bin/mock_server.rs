//! A mock `/v1/systemone` server for local testing of the `rone` CLI.
//!
//! Serves the deterministic answers from `rone::mock` in place of a real
//! System One model, so the full CLI path (request, network, rendering,
//! exit codes) can be exercised without an inference server.
//!
//! Usage:
//!
//!     cargo run --bin mock_server                 # http://127.0.0.1:8123
//!     RONE_MOCK_PORT=9000 cargo run --bin mock_server

use std::collections::BTreeMap;
use std::net::SocketAddr;

use axum::Router;
use axum::extract::Json;
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use rone::mock::generate_answers;
use rone::{QuestionDef, QuestionType};

type ApiError = (StatusCode, Json<Value>);

fn bad_request(message: &str) -> ApiError {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": message })),
    )
}

/// Rebuild preset questions from the request shape
/// `{"type", "instructions", "criteria"}`, where the `criteria` keys are the
/// options of a choice question.
fn questions_from_request(
    questions: &serde_json::Map<String, Value>,
) -> Result<BTreeMap<String, QuestionDef>, ApiError> {
    let mut out = BTreeMap::new();
    for (id, q) in questions {
        let kind = match q.get("type").and_then(Value::as_str) {
            Some("choice") => QuestionType::Choice,
            Some("score") => QuestionType::Score,
            Some("noul") => QuestionType::Noul,
            Some(other) => {
                return Err(bad_request(&format!(
                    "question '{id}': unknown type '{other}'"
                )));
            }
            None => return Err(bad_request(&format!("question '{id}': missing type"))),
        };
        let instructions = q
            .get("instructions")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_request(&format!("question '{id}': missing instructions")))?
            .to_string();
        let mut options = Vec::new();
        let mut criteria = BTreeMap::new();
        if let Some(c) = q.get("criteria").and_then(Value::as_object) {
            for (opt, description) in c {
                options.push(opt.clone());
                if let Some(d) = description.as_str() {
                    criteria.insert(opt.clone(), d.to_string());
                }
            }
        }
        out.insert(
            id.clone(),
            QuestionDef {
                kind,
                instructions,
                options,
                criteria,
            },
        );
    }
    Ok(out)
}

/// `POST /v1/systemone`: answer the questions deterministically.
async fn systemone(Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let state = body
        .get("state")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request("missing 'state'"))?;
    let questions = body
        .get("questions")
        .and_then(Value::as_object)
        .ok_or_else(|| bad_request("missing 'questions' object"))?;
    let questions = questions_from_request(questions)?;
    let model = body.get("model").and_then(Value::as_str).unwrap_or("mock");
    Ok(Json(json!({
        "model": model,
        "state": state,
        "answers": generate_answers(state, &questions),
    })))
}

/// `GET /v1/models`: a minimal OpenAI-compatible model list.
async fn models() -> Json<Value> {
    Json(json!({
        "object": "list",
        "data": [
            { "id": "mock", "object": "model" }
        ]
    }))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port: u16 = std::env::var("RONE_MOCK_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8123);
    let app = Router::new()
        .route("/v1/systemone", post(systemone))
        .route("/v1/models", get(models));
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(addr).await?;
    println!("rone mock server: http://{addr} (POST /v1/systemone, GET /v1/models)");
    axum::serve(listener, app).await?;
    Ok(())
}
