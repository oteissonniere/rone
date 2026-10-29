# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-07-14

### Added

- `rone ask`: ask a System One decision model using a reusable preset
- `rone batch`: ask a model over many inputs (one state per line)
- `rone eval`: evaluate a preset over a labeled dataset and report accuracy
- `rone models`: list the models available on the configured endpoint
- `rone list-presets` and `rone list-profiles`
- `rone mcp`: run rone as an MCP (Model Context Protocol) server over stdio
- Six built-in presets: compliance, escalation, intent, sentiment, severity, triage
- Profile-based configuration (`rone.toml`): host, model and API key per profile
- Colored, human-friendly output; disable with `--no-color` or `NO_COLOR`
- JSON output on every command with `--json`
- `mock_server`: a deterministic mock of the `/v1/systemone` endpoint for offline tests
- Sample eval dataset in `examples/evals/triage.json`
