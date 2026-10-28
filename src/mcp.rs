//! MCP (Model Context Protocol) server: expose the Rone presets as MCP tools
//! over stdio, so an AI agent can call `list_presets`, `ask` and `batch`.
//!
//! Transport: JSON-RPC 2.0, one message per line on stdin/stdout. All logs go
//! to stderr so they never break the stdout protocol.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use rone::{
    below_threshold, build_request, color::ColorMode, extract_answers, list_presets, load_preset,
    render_answers, render_batch, validate,
};

/// Protocol versions this server understands, newest first.
const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// Version we report when the client asks for one we do not support.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// The text an MCP tool produced, plus whether it should be flagged as an error.
struct ToolOutcome {
    text: String,
    /// `true` when the call failed, or when at least one answer is below the
    /// confidence threshold.
    below: bool,
}

/// Run the MCP server loop: read one JSON-RPC message per line from stdin,
/// write the response (if any) to stdout.
pub async fn run() -> Result<()> {
    eprintln!("rone: MCP server running on stdio");
    let mut reader = BufReader::new(std::io::stdin()).lines();
    let mut out = std::io::stdout();
    loop {
        match reader.next() {
            Some(Ok(line)) => {
                if let Some(resp) = handle(&line).await {
                    writeln!(out, "{resp}")?;
                    out.flush()?;
                }
            }
            Some(Err(e)) => return Err(e).context("reading stdin"),
            None => break,
        }
    }
    Ok(())
}

/// Handle one JSON-RPC message. Returns the response to send, or `None` for
/// notifications (which expect no response).
async fn handle(msg: &str) -> Option<Value> {
    let value: Value = match serde_json::from_str(msg) {
        Ok(v) => v,
        Err(e) => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": -32700, "message": format!("Parse error: {e}") },
            }));
        }
    };

    let method = value.get("method").and_then(Value::as_str).unwrap_or("");
    let id = value.get("id").cloned();
    let params = value.get("params").cloned().unwrap_or(Value::Null);

    // Notifications carry no `id` and expect no response.
    if value.get("id").is_none() {
        match method {
            "notifications/initialized" | "notifications/cancelled" => {}
            other => eprintln!("rone: ignoring unknown notification '{other}'"),
        }
        return None;
    }

    let result: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(initialize_result(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list()),
        "tools/call" => Ok(call_tool(&params).await),
        other => Err((-32601, format!("Method not found: {other}"))),
    };

    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        }),
    })
}

/// The `initialize` result: echo the client's protocol version if we support
/// it, otherwise fall back to the newest one we do.
fn initialize_result(params: &Value) -> Value {
    let client_version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("");
    let protocol_version = if SUPPORTED_VERSIONS.contains(&client_version) {
        client_version.to_string()
    } else {
        PROTOCOL_VERSION.to_string()
    };
    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "rone", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Rone: call System One decision models. Use list_presets to \
         discover available presets, then ask (one state) or batch (many states) \
         a preset."
    })
}

/// The `tools/list` result.
fn tools_list() -> Value {
    let common = json!({
        "vars": {
            "type": "object",
            "description": "Variables substituted into instructions as {{name}}."
        },
        "min_confidence": {
            "type": "number",
            "minimum": 0.0,
            "maximum": 1.0,
            "description": "Only accept answers with at least this much confidence."
        },
        "model": { "type": "string", "description": "Model name sent in the request." },
        "base_url": {
            "type": "string",
            "description": "Base URL of the /v1/systemone-compatible endpoint."
        },
        "json": {
            "type": "boolean",
            "description": "Return machine-readable JSON instead of a table."
        }
    });
    json!({
        "tools": [
            {
                "name": "list_presets",
                "description": "List the preset names available in presets/.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "ask",
                "description": "Ask a System One model using a preset, for a single state.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "preset": { "type": "string", "description": "Preset name (presets/<name>.toml)." },
                        "state": { "type": "string", "description": "The input text to analyze." },
                        "vars": common["vars"],
                        "min_confidence": common["min_confidence"],
                        "model": common["model"],
                        "base_url": common["base_url"],
                        "json": common["json"]
                    },
                    "required": ["preset", "state"]
                }
            },
            {
                "name": "batch",
                "description": "Ask a System One model over many states (one state per line).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "preset": { "type": "string", "description": "Preset name (presets/<name>.toml)." },
                        "states": {
                            "type": ["array", "string"],
                            "description": "Array of states, or a multi-line string with one state per line."
                        },
                        "vars": common["vars"],
                        "min_confidence": common["min_confidence"],
                        "model": common["model"],
                        "base_url": common["base_url"],
                        "json": common["json"]
                    },
                    "required": ["preset", "states"]
                }
            }
        ]
    })
}

/// Dispatch a `tools/call` request to the named tool.
async fn call_tool(params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let outcome = match name {
        "list_presets" => run_list_presets(&args).await,
        "ask" => run_ask(&args).await,
        "batch" => run_batch(&args).await,
        other => ToolOutcome {
            text: format!("unknown tool '{other}'"),
            below: true,
        },
    };
    tool_result(outcome)
}

/// Wrap a tool outcome in the MCP `tools/call` result shape.
fn tool_result(outcome: ToolOutcome) -> Value {
    json!({
        "content": [ { "type": "text", "text": outcome.text } ],
        "isError": outcome.below
    })
}

/// `list_presets`: no arguments, returns one preset name per line.
async fn run_list_presets(_args: &Value) -> ToolOutcome {
    match list_presets() {
        Ok(names) => ToolOutcome {
            text: names.join("\n"),
            below: false,
        },
        Err(e) => ToolOutcome {
            text: format!("Error: {e:#}"),
            below: true,
        },
    }
}

/// `ask`: run one state through a preset.
async fn run_ask(args: &Value) -> ToolOutcome {
    match ask_inner(args).await {
        Ok(outcome) => outcome,
        Err(e) => ToolOutcome {
            text: format!("Error: {e:#}"),
            below: true,
        },
    }
}

async fn ask_inner(args: &Value) -> Result<ToolOutcome> {
    let preset_name = arg_str(args, "preset")?;
    let state = arg_str(args, "state")?;
    let common = common_args(args)?;
    let vars = arg_vars(args)?;

    let preset = load_preset(&preset_name)?;
    validate(&preset)?;
    let request = build_request(&common.model, &state, &preset, &vars)?;
    let response = send(&common.base_url, &request).await?;
    let answers = extract_answers(&response);

    let mut text = render_answers(&answers, common.as_json, ColorMode::off());
    let mut below = false;
    if let Some(min) = common.min_confidence {
        let failed = below_threshold(&answers, min);
        if !failed.is_empty() {
            below = true;
            let mut warnings = String::new();
            for (id, conf) in &failed {
                match conf {
                    Some(c) => warnings.push_str(&format!(
                        "warning: answer '{id}': confidence {c:.2} is below threshold {min:.2}\n"
                    )),
                    None => warnings.push_str(&format!(
                        "warning: answer '{id}': no confidence reported, threshold is {min:.2}\n"
                    )),
                }
            }
            text = format!("{warnings}{text}");
        }
    }
    Ok(ToolOutcome { text, below })
}

/// `batch`: run many states through a preset.
async fn run_batch(args: &Value) -> ToolOutcome {
    match batch_inner(args).await {
        Ok(outcome) => outcome,
        Err(e) => ToolOutcome {
            text: format!("Error: {e:#}"),
            below: true,
        },
    }
}

async fn batch_inner(args: &Value) -> Result<ToolOutcome> {
    let preset_name = arg_str(args, "preset")?;
    let states = arg_states(args)?;
    let common = common_args(args)?;
    let vars = arg_vars(args)?;

    let preset = load_preset(&preset_name)?;
    validate(&preset)?;

    let mut results: Vec<(String, BTreeMap<String, Value>)> = Vec::new();
    let mut any_below = false;
    let mut warnings = String::new();
    for (i, state) in states.iter().enumerate() {
        let request = build_request(&common.model, state, &preset, &vars)?;
        let response = send(&common.base_url, &request).await?;
        let answers = extract_answers(&response);
        if let Some(min) = common.min_confidence {
            for (id, conf) in &below_threshold(&answers, min) {
                any_below = true;
                match conf {
                    Some(c) => warnings.push_str(&format!(
                        "warning: input #{}: answer '{id}': confidence {c:.2} is below threshold {min:.2}\n",
                        i + 1
                    )),
                    None => warnings.push_str(&format!(
                        "warning: input #{}: answer '{id}': no confidence reported, threshold is {min:.2}\n",
                        i + 1
                    )),
                }
            }
        }
        results.push((state.clone(), answers));
    }

    // The latency line is a CLI-only feature: in JSON mode the batch is
    // rendered through render_batch (the zero duration is ignored there),
    // and in text mode the numbered blocks are rendered directly so the
    // MCP output stays unchanged.
    let text = if common.as_json {
        let with_latency: Vec<(String, BTreeMap<String, Value>, Duration)> = results
            .iter()
            .map(|(state, answers)| (state.clone(), answers.clone(), Duration::ZERO))
            .collect();
        render_batch(&with_latency, true, ColorMode::off())
    } else {
        results
            .iter()
            .enumerate()
            .map(|(i, (state, answers))| {
                format!(
                    "#{}: {}\n{}",
                    i + 1,
                    state,
                    render_answers(answers, false, ColorMode::off())
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut text = text;
    let below = any_below;
    if below {
        text = format!("{warnings}{text}");
    }
    Ok(ToolOutcome { text, below })
}

/// Required string argument: trimmed, non-empty.
fn arg_str(args: &Value, name: &str) -> Result<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .with_context(|| format!("missing required argument '{name}'"))
}

/// Optional string argument: trimmed, non-empty, else `None`.
fn arg_opt_str(args: &Value, name: &str) -> Option<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Optional `vars` object: a JSON object of string values, ordered.
fn arg_vars(args: &Value) -> Result<BTreeMap<String, String>> {
    let Some(v) = args.get("vars") else {
        return Ok(BTreeMap::new());
    };
    let Some(obj) = v.as_object() else {
        bail!("'vars' must be an object of string values");
    };
    let mut map = BTreeMap::new();
    for (key, val) in obj {
        let s = val
            .as_str()
            .with_context(|| format!("variable '{key}' must be a string"))?;
        map.insert(key.clone(), s.to_string());
    }
    Ok(map)
}

/// Required `states` argument: an array of strings, or a multi-line string.
fn arg_states(args: &Value) -> Result<Vec<String>> {
    let v = args
        .get("states")
        .with_context(|| "missing required argument 'states'")?;
    let mut states: Vec<String> = Vec::new();
    if let Some(arr) = v.as_array() {
        for item in arr {
            let s = item
                .as_str()
                .with_context(|| "each state must be a string")?;
            let s = s.trim();
            if !s.is_empty() {
                states.push(s.to_string());
            }
        }
    } else if let Some(s) = v.as_str() {
        for line in s.lines() {
            let t = line.trim();
            if !t.is_empty() {
                states.push(t.to_string());
            }
        }
    } else {
        bail!("'states' must be an array of strings or a multi-line string");
    }
    if states.is_empty() {
        bail!("'states' must not be empty");
    }
    Ok(states)
}

/// The arguments shared by `ask` and `batch`, with their defaults.
struct CommonArgs {
    model: String,
    base_url: String,
    as_json: bool,
    min_confidence: Option<f64>,
}

fn common_args(args: &Value) -> Result<CommonArgs> {
    let model = arg_opt_str(args, "model").unwrap_or_else(|| "laya".to_string());
    let base_url =
        arg_opt_str(args, "base_url").unwrap_or_else(|| "http://localhost:1234/v1".to_string());
    let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);
    let min_confidence = match args.get("min_confidence") {
        Some(v) => {
            let f = v
                .as_f64()
                .with_context(|| "'min_confidence' must be a number between 0 and 1")?;
            if !(0.0..=1.0).contains(&f) {
                bail!("'min_confidence' must be between 0 and 1");
            }
            Some(f)
        }
        None => None,
    };
    Ok(CommonArgs {
        model,
        base_url,
        as_json,
        min_confidence,
    })
}

/// POST a `/v1/systemone` request. (Duplicated from the CLI on purpose: the
/// library stays HTTP-free.)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initialize_echoes_supported_version() {
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#;
        let resp = handle(msg).await.unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(resp["result"]["serverInfo"]["name"], "rone");
        assert_eq!(resp["id"], 1);
    }

    #[tokio::test]
    async fn initialize_falls_back_on_unknown_version() {
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#;
        let resp = handle(msg).await.unwrap();
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn tools_list_has_the_three_tools() {
        let msg = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let resp = handle(msg).await.unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["list_presets", "ask", "batch"]);
    }

    #[tokio::test]
    async fn ping_returns_empty_object() {
        let msg = r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#;
        let resp = handle(msg).await.unwrap();
        assert_eq!(resp["result"], json!({}));
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let msg = r#"{"jsonrpc":"2.0","id":4,"method":"bogus"}"#;
        let resp = handle(msg).await.unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn unknown_tool_is_error_result() {
        let msg = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#;
        let resp = handle(msg).await.unwrap();
        assert_eq!(resp["result"]["isError"], true);
        assert!(
            resp["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("unknown tool")
        );
    }

    #[tokio::test]
    async fn ask_without_preset_is_error() {
        let msg = r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"ask","arguments":{}}}"#;
        let resp = handle(msg).await.unwrap();
        assert_eq!(resp["result"]["isError"], true);
        assert!(
            resp["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("preset")
        );
    }

    #[tokio::test]
    async fn batch_without_states_is_error() {
        let msg = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"batch","arguments":{"preset":"triage"}}}"#;
        let resp = handle(msg).await.unwrap();
        assert_eq!(resp["result"]["isError"], true);
        assert!(
            resp["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("states")
        );
    }

    #[tokio::test]
    async fn bad_json_is_parse_error() {
        let resp = handle("not json").await.unwrap();
        assert_eq!(resp["error"]["code"], -32700);
        assert_eq!(resp["id"], Value::Null);
    }

    #[tokio::test]
    async fn notification_returns_no_response() {
        let msg = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(handle(msg).await.is_none());
    }
}
