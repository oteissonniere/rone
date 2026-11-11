# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2026-11-17

### Added

- `rone mcp` now supports the profiles defined in `rone.toml`: a new `list_profiles` MCP tool lists them, and `ask` and `batch` accept a `profile` argument (with the `RONE_PROFILE` environment variable as fallback). Per-call target resolution follows the same rules as the CLI commands — `profile` argument, then `RONE_PROFILE`, then the config file's `default_profile`, then the built-in defaults.
- Per-call `model`, `base_url` and `min_confidence` arguments override the profile's values for that call; a profile is also the only way to carry an `api_key` (sent as a bearer token) and a request `timeout` to the MCP server — endpoints requiring authentication are now reachable from it.

### Fixed

- Unknown profile names in MCP mode are reported as a readable tool error ("no rone.toml found") instead of silently falling back to the built-in defaults.

## [0.2.2] - 2026-11-10

### Fixed

- `rone upgrade` (script installs): the latest-release check now sends a `User-Agent` header with its GitHub request, as required by the GitHub API (requests without one are rejected with HTTP 403). Without it, the up-to-date check silently failed and the installer was re-run every time.

## [0.2.1] - 2026-11-10

### Added

- `rone upgrade` (script installs): before re-running the installer, it now checks the latest published release on GitHub and stops with a "nothing to do" message when rone is already up to date; if the check cannot be done without network, it proceeds with the installer anyway.

## [0.2.0] - 2026-11-10

### Added

- `rone upgrade` subcommand: detects how rone is installed (installer script vs Homebrew) and self-upgrades accordingly — re-runs the installer script from GitHub for script installs, and prints the one-liner to run (`brew upgrade rone`) for Homebrew installs instead of executing it.

### Changed

- README: added a dedicated section for installing with Homebrew on macOS (tap, `brew trust`, install), an "Upgrading" section documenting `rone upgrade` and `brew upgrade rone`, brew uninstall steps in the uninstall section, and a self-upgrade item in the progress list.

## [0.1.3] - 2026-11-09

### Fixed

- `scripts/install.sh`: re-running the installer no longer accumulates versions — every previously installed `~/.local/share/rone/vX.Y.Z` directory is removed, so only the newest one remains on disk (the old version stays untouched if a run fails).
- Homebrew formula (`oteissonniere/homebrew-rone` tap): bundled presets were being extracted nested in `bin/presets/presets/`, where rone could not find them at runtime; they now land directly in `bin/presets`.

### Changed

- README: expanded coverage of the supported platforms, per-platform install and uninstall steps, and details on the MCP server.

## [0.1.2] - 2026-11-05

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
