# Rone

**Rone** (*Rust One*, for *System One*) is a small Rust tool that makes
[System One](#background) decision models (Clef, Laya, Jev and compatible
models) pleasant to use from the command line.

> **Status: working skeleton.** A first `rone ask` CLI is implemented and
> tested against a live `/v1/systemone` endpoint. The interface shown below is
> still a draft and names/flags may change.

## Why

System One models don't write text. You send them a *state* and a set of
*typed questions*, and they return a probability for every allowed option.
That is great for routing, triage and guardrails, but the request format is a
fairly verbose JSON document, and writing it by hand is much less natural than
talking to a chat LLM.

Rone sits between you and the model. It should take care of the JSON so you
only have to provide what actually changes: the input and a few parameters.

## Background

System One models answer constrained questions in a single forward pass, with
no text generation. Questions come in three types:

- `choice`: pick exactly one option from a list
- `score`: rate something on a scale
- `noul`: a yes/no probability

Most implementations follow the request shape popularised by TypeSafe's Jev
(`POST /v1/systemone`): a model name, a `state`, and a map of `questions`,
answered with `answers` keyed by question id.

A raw request looks roughly like this:

```json
{
  "model": "laya",
  "state": "I was billed twice, please refund the duplicate.",
  "questions": {
    "department": {
      "type": "choice",
      "instructions": "Who should handle this?",
      "criteria": {
        "billing": "billing issues",
        "technical": "technical issues",
        "sales": "sales issues"
      }
    }
  }
}
```

> Note: the endpoint expects `criteria` to be an **object** (option →
> description), not an array. This was verified against a live server.

The response is keyed by question id, with the shape below (verified against a
live endpoint):

```json
{
  "answers": {
    "department": {
      "type": "choice",
      "choice": "billing",
      "confidence": 0.954,
      "probabilities": {
        "billing": 0.970,
        "technical": 0.022,
        "sales": 0.008
      }
    },
    "needs_human": {
      "type": "noul",
      "noul": 0.344
    }
  }
}
```

## Goals

- Talk to any `/v1/systemone`-compatible endpoint (LM Studio or another local
  server, a self-hosted gateway, a hosted API), with a configurable base URL.
- Reusable **presets**: describe your questions once in a small config file,
  then reuse them by name.
- **Parametrizable input**: text from an argument, a file or stdin, plus
  variables injected into the question text.
- **Readable output**: a table of options with their probabilities, and a
  `--json` mode for scripts.
- **Helpful validation**: catch common mistakes before sending (empty option
  lists, too many options in one `choice` question, low confidence), with clear
  messages instead of raw server errors.

## Progress

- **Working CLI** (`src/main.rs`): `rone ask` built on `clap`, with `tokio` /
  `reqwest` for the HTTP call and `serde` / `serde_json` / `toml` for
  (de)serialization.
- **Presets**: reusable question sets loaded from `presets/<name>.toml`
  (see `presets/triage.toml`).
- **Option descriptions**: an optional `criteria` table in a preset
  describes each `choice` option; without one, the option name is its own
  description.
- **Parametrizable input**: text from `--text`, `--file`, or stdin.
- **Request building**: sends a `/v1/systemone` request with `criteria` as an
  object (option → description), as the endpoint expects.
- **Readable output**: a probability table (`*` marks the chosen option) and a
  `--json` mode for scripts.
- **Validation**: rejects empty `choice` option lists and criteria entries
  for unknown options, and warns when a single `choice` question has many
  options.
- **Confidence threshold**: `--min-confidence` accepts answers only when the
  model is confident enough; below the threshold it warns and exits with
  code 3, so scripts can act conditionally.
- **Verified** against a live `/v1/systemone` endpoint (LM Studio).

Still to come (see [Possible later steps](#possible-later-steps)): a reusable
library crate, batch mode and MCP server mode.

## Draft interface

Illustrative only; names and flags may change.

```toml
# presets/triage.toml
[questions.department]
type = "choice"
instructions = "Who should handle this?"
options = ["billing", "technical", "sales"]

[questions.department.criteria]
# optional: describe each option for the model
billing = "billing issues"
technical = "technical issues"
sales = "sales issues"

[questions.needs_human]
type = "noul"
instructions = "A human must handle this."
```

```bash
# one-off question
rone ask --preset triage --text "I was billed twice, please refund"

# from a file or stdin, against a specific endpoint
cat ticket.txt | rone ask --preset triage --base-url http://localhost:1234/v1

# machine-readable output
rone ask --preset triage --file ticket.txt --json

# act only when the model is confident enough (exit code 3 otherwise)
rone ask --preset triage --text "I was billed twice" --min-confidence 0.9
```

## Possible later steps

- Reusable library crate alongside the CLI
- Confidence threshold mode (act only when the model is confident enough)
- Batch mode over many inputs
- MCP server mode, so agents can call your presets as tools

## Related projects

Rone is not the first tool around System One models. Related work:

- [Laya](https://github.com/NandhaKishorM/laya): the open non-autoregressive decision model
- [`laya-decision`](https://docs.rs/crate/laya-decision/latest): a pure-Rust port of Laya that runs inference on candle
- [`jigor-cli`](https://docs.rs/crate/jigor-cli/latest): a Rust CLI and HTTP gateway for several System One models
- [`opendecider`](https://pypi.org/project/opendecider-client/): Python server and client, Jev-compatible, including LM Studio setups
- [Cloudflare Clef](https://developers.cloudflare.com/workers-ai/models/clef/) and [TypeSafe Jev](https://docs.typesafe.ai/): the models and API this protocol comes from

Rone does not run models itself. It focuses on the client side: making the
request and the result easy for humans to work with.

## Known caveats of System One models

- Calibration varies by model and task; treat probabilities as signals, not guarantees.
- Questions with many options in a single `choice` can degrade results.
- Support for the `/v1/systemone` endpoint depends on your inference server and
  model format. Check your server's documentation.

## Contributing

Ideas, issues and pull requests are welcome once the project skeleton is in place.

## License

MIT. See [LICENSE](LICENSE).

Rone is an independent project and is not affiliated with TypeSafe,
Cloudflare, Convai or any model author mentioned above.
