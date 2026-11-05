# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- `ask --profile <name>` (single profile, no comparison) is now actually used; it was silently ignored and the configured default profile — or the built-in localhost endpoint when there is no config file — was contacted instead.
- Requesting a profile that no `rone.toml` defines now fails with a clear error naming the profile and explaining where to create it, instead of silently falling back to the built-in default endpoint.

## [0.1.1] - 2026-11-04

### Added

- `scripts/install.sh`: one-liner end-user installer (`curl -fsSL ... | sh`) that detects the platform, verifies the release tarball's SHA-256 checksum and installs into `~/.local/share/rone/<version>/` with a symlink in `~/.local/bin`; supports pinned versions, and a `--dry-run` preview that resolves the latest release online (fully offline with a pinned version).

### Fixed

- Presets bundled next to the binary are found again when `rone` is run through a symlink (e.g. the installer's `~/.local/bin/rone`): the executable path is now resolved before looking for `presets/`.

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
- `scripts/release.sh`: builds and packages a distributable tarball in
  `dist/` (binary + presets + README/LICENSE/CHANGELOG) with a SHA-256
  sidecar for download verification
- GitHub Actions workflow that, on a `v*` tag matching the Cargo.toml
  version, builds macOS arm64/x86_64 and Linux x86_64/arm64 and publishes a
  GitHub Release with each tarball and `.sha256` sidecar as assets
