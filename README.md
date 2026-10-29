# Rone

**Rone** (*Rust One*, for *System One*) is a small Rust tool that makes
[System One](#background) decision models (Clef, Laya, Jev and compatible
models) pleasant to use from the command line.

> **Status: working skeleton.** A `rone ask` CLI and a `rone mcp` server are
> implemented and tested against a live `/v1/systemone` endpoint, plus a
> deterministic mock server for offline testing. The interface shown below
> is still a draft and names/flags may change.

## Why

System One models don't write text. You send them a *state* and a set of
*typed questions*, and they return a probability for every allowed option.
That is great for routing, triage and guardrails, but the request format is a
fairly verbose JSON document, and writing it by hand is much less natural than
talking to a chat LLM.

Rone sits between you and the model. It should take care of the JSON so you
only have to provide what actually changes: the input and a few parameters.

## Installation

Rone is a single binary built with Rust. From a checkout of this repo:

```bash
cargo install --path .     # installs rone (and mock_server) into ~/.cargo/bin
```

or build it in place:

```bash
cargo build --release      # binary in target/release/rone
```

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
- **Composable presets**: `--preset a,b` merges the question sets of several
  presets into a single request, so guardrails can be chained (triage +
  compliance, triage + severity, ...).
- **Parametrizable input**: text from an argument, a file or stdin, plus
  variables injected into the question text.
- **Readable output**: a table of options with their probabilities, and a
  `--json` mode for scripts.
- **Side-by-side comparison**: `--profile a,b` sends the same preset to
  several servers/models at once and renders their answers side by side.
- **Helpful validation**: catch common mistakes before sending (empty option
  lists, too many options in one `choice` question, low confidence), with clear
  messages instead of raw server errors.
- **Offline debugging**: `--dry-run` prints the exact request that would be
  sent, without any network call.

## Progress

- **Working CLI** (`src/main.rs`): `rone ask` built on `clap`, with `tokio` /
  `reqwest` for the HTTP call and `serde` / `serde_json` / `toml` for
  (de)serialization.
- **Library** (`src/lib.rs`): preset parsing, validation, request building,
  answer extraction and rendering are exposed as a public Rust API in the same
  `rone` crate; the binary is a thin CLI on top of it, with no HTTP in the
  library.
- **Presets**: reusable question sets loaded from `presets/<name>.toml`
  (see `presets/triage.toml`, `presets/severity.toml`,
  `presets/escalation.toml`, `presets/intent.toml`,
  `presets/sentiment.toml` and `presets/compliance.toml`);
  `rone list-presets` shows the presets of the current `presets/` directory.
- **Composable presets**: `--preset a,b` (in `ask`) merges the questions of
  several presets into one `/v1/systemone` request. A question id defined in
  several presets is accepted only when the definitions are identical (the
  same question, deduplicated); conflicting definitions are a clear error.
- **Option descriptions**: an optional `criteria` table in a preset
  describes each `choice` option; without one, the option name is its own
  description.
- **Parametrizable input**: text from `--text`, `--file`, or stdin.
- **Request building**: sends a `/v1/systemone` request with `criteria` as an
  object (option → description), as the endpoint expects.
- **Readable output**: a probability table (`*` marks the chosen option) and a
  `--json` mode for scripts. Text output is colored on a terminal (bold ids,
  green/yellow/red magnitudes); piping disables colors, `--no-color` forces
  them off, and the `NO_COLOR` environment variable is honored.
- **Request latency**: in text mode, the duration of each request is shown
  under the answer table (`elapsed: 842 ms`, seconds above 1 s); in batch
  mode one line per input plus a `total:` line for the whole run.
- **Validation**: rejects empty `choice` option lists and criteria entries
  for unknown options, and warns when a single `choice` question has many
  options.
- **Confidence threshold**: `--min-confidence` accepts answers only when the
  model is confident enough; below the threshold it warns and exits with
  code 3, so scripts can act conditionally.
- **Variables**: `--var NAME=VALUE` (repeatable) injects values into question
  instructions via `{{name}}` placeholders; an unknown placeholder is a
  clear error before anything is sent.
- **Dry run**: `--dry-run` prints the endpoint URL and the exact request
  body (after preset, variable and profile resolution) without sending
  it; in comparison mode one block is printed per profile.
- **Batch mode**: `rone batch` runs the same preset over many inputs (one
  state per line, from `--file` or stdin); `--json` emits an array of
  `{"input", "answers"}` objects for scripts.
- **Profiles**: a config file (`rone.toml`) defines named server/model
  profiles (`base_url`, `model`, `min_confidence`, `api_key`, `timeout`);
  pick one with `--profile` (or the `RONE_PROFILE` environment variable) or
  set a `default_profile`. Command-line flags override the profile. `rone list-profiles` shows the profiles of the loaded
  config file (the default is marked with `*`; literal `api_key` values are
  hidden, `env:VAR` references are shown). Passing a comma-separated list
  (`--profile a,b`) sends the same preset to each listed profile and renders
  their answers side by side, one labeled block per profile; JSON mode emits
  an array of `{profile, model, answers}` objects.
- **MCP server mode**: `rone mcp` serves the presets as MCP tools over stdio
  (`list_presets`, `ask`, `batch`), so AI agents can call them.
- **Model listing**: `rone models` queries the `/models` endpoint of the
  selected profile (or `--base-url`) and lists the available models, one per
  line (bold id, name and description when the server sends them); the
  profile's `api_key` and `timeout` are honored.
- **Mock server** (`src/bin/mock_server.rs`): a local
  `/v1/systemone`-compatible endpoint with deterministic answers (a pure
  function of the input), so the CLI can be exercised end to end without an
  inference server; the integration tests in `tests/` run against it.
- **Examples**: sample tickets in `examples/tickets.txt` for batch mode.
- **Verified** against a live `/v1/systemone` endpoint (LM Studio).

## Roadmap

- Set up the GitHub repository and publish on crates.io
- More presets for common use cases (sentiment, compliance, ...)
- Packaging (install script, Homebrew tap)
- First-class example of a multi-question preset with `--var` variables

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

# merge several presets into a single request (e.g. triage + compliance guardrail)
rone ask --preset triage,compliance --text "I was billed twice, please refund"

# list the presets available in presets/
rone list-presets

# list the models served by a profile
rone models --profile local

# from a file or stdin, against a specific endpoint
cat ticket.txt | rone ask --preset triage --base-url http://localhost:1234/v1

# machine-readable output
rone ask --preset triage --file ticket.txt --json

# plain output, no colors even on a terminal
rone ask --preset triage --text "I was billed twice" --no-color

# act only when the model is confident enough (exit code 3 otherwise)
rone ask --preset triage --text "I was billed twice" --min-confidence 0.9

# compare two profiles (two servers/models) side by side
rone ask --preset triage --text "I was billed twice" --profile local,production

# inject variables into question instructions ({{name}} placeholders in the preset)
rone ask --preset triage --text "I was billed twice" --var team=support --var lang=fr

# show the exact request that would be sent, without sending it
rone ask --preset triage --text "I was billed twice" --dry-run

# many inputs at once: one state per line, from a file or stdin
rone batch --preset triage --file examples/tickets.txt
cat examples/tickets.txt | rone batch --preset triage --json
```

### Configuration file

Rone reads an optional TOML config file to define named **profiles** — a
`base_url`, a `model`, and optional `min_confidence`, `api_key` and
`timeout` bundled together. The first existing file in this order is used:

1. `--config <path>` (explicit)
2. `$RONE_CONFIG`
3. `./rone.toml` (current directory)
4. `~/.config/rone/rone.toml` (or `$XDG_CONFIG_HOME/rone/rone.toml`)

If none exists, rone uses its built-in defaults (`http://localhost:1234/v1`,
model `laya`).

```toml
# rone.toml
default_profile = "local"

[profiles.local]
base_url = "http://localhost:1234/v1"
model = "laya"
min_confidence = 0.8

[profiles.production]
base_url = "https://api.example.com/v1"
model = "laya-2"
# Bearer token; "env:VAR" reads it from the environment
api_key = "env:RONE_API_KEY"
# request timeout in seconds
timeout = 30
```

Pick a profile with `--profile`, or set the `RONE_PROFILE` environment
variable; without either, `default_profile` is used (and without that, the
built-in defaults). An explicit `--profile` flag wins over the environment
variable, and explicit flags always win over the profile. A comma-separated
list (`--profile a,b`) compares the listed profiles side by side:

```bash
# use the "production" profile
rone ask --preset triage --profile production --text "I was billed twice"

# the profile sets base_url/model, but a flag overrides it
rone ask --preset triage --profile local --model laya-2 --text "..."

# point at a specific config file
rone ask --preset triage --config ./ci.rone.toml --profile production --text "..."

# list the profiles of the loaded config file
rone list-profiles
```

`api_key` accepts a literal token or an `env:VAR` reference, resolved from
the environment when the profile is used, so the secret stays out of the
file. `timeout` is a number of seconds. Both can be overridden on the
command line with `--api-key` and `--timeout`.

See [`config.example.toml`](config.example.toml) for a full annotated example.

### MCP server

`rone mcp` serves the presets as MCP tools over stdio, for AI agents that
speak MCP. Tools: `list_presets`, `ask`, `batch`.

Client configuration (Claude Desktop, Cursor, or any MCP client):

```json
{
  "mcpServers": {
    "rone": {
      "command": "rone",
      "args": ["mcp"]
    }
  }
}
```

Presets resolve from the `presets/` directory of the working directory the
server is started in.

### Mock server

A small `/v1/systemone`-compatible server (`src/bin/mock_server.rs`) answers
with deterministic, pseudo-random values generated from the input (FNV-1a),
so the full CLI path — request, network, rendering, exit codes — can be
exercised without an inference server:

```bash
cargo run --bin mock_server              # http://127.0.0.1:8123
RONE_MOCK_PORT=9000 cargo run --bin mock_server   # another port
```

Then point the CLI at it:

```bash
rone ask --preset triage --text "I was billed twice" \
    --base-url http://127.0.0.1:8123/v1 --model mock

rone batch --preset triage --file examples/tickets.txt \
    --base-url http://127.0.0.1:8123/v1 --model mock
```

Answers are a pure function of the input: the same state and questions always
produce the same answers. `choice` answers carry a confidence between 0.85
and 0.99, `noul` answers a probability between 0.10 and 0.899, and `score`
answers a score on the default 1..5 scale — so both sides of a
`--min-confidence` threshold are reachable. The integration tests in
`tests/mock_integration.rs` pin the exact values.

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

Ideas, issues and pull requests are welcome.

Development loop — the same checks CI runs on every push and pull request:

```bash
cargo fmt && cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

A few conventions:

- **Presets** live in `presets/<name>.toml`, one file per use case;
  `rone list-presets` picks them up from the working directory.
- **Profiles** belong in a local `rone.toml` (gitignored);
  `config.example.toml` is the annotated reference for the format.
- **Integration tests** in `tests/` run against the bundled mock server
  (`src/bin/mock_server.rs`), so no inference server is needed to verify a
  change.

## License

MIT. See [LICENSE](LICENSE).

Rone is an independent project and is not affiliated with TypeSafe,
Cloudflare, Convai or any model author mentioned above.
