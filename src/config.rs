//! Rone configuration: named server/model profiles.
//!
//! A config file is a TOML file mapping profile names to a `base_url`, a
//! `model` and an optional `min_confidence`. The CLI picks one profile (by
//! `--profile`, or the config's `default_profile`) and lets explicit
//! command-line flags override any of its fields.
//!
//! No HTTP, no CLI: just data structures and file discovery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::color::ColorMode;

/// Built-in default base URL, used when neither a profile nor a flag sets it.
pub const DEFAULT_BASE_URL: &str = "http://localhost:1234/v1";
/// Built-in default model name.
pub const DEFAULT_MODEL: &str = "laya";

fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

fn default_model() -> String {
    DEFAULT_MODEL.to_string()
}

/// A server/model profile: where to send requests and with what model.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// Optional default confidence threshold (0..=1) for this profile.
    #[serde(default)]
    pub min_confidence: Option<f64>,
    /// API key for the inference endpoint, sent as a Bearer token. May be a
    /// literal value or `env:VAR` to read it from the environment.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Request timeout in seconds.
    #[serde(default)]
    pub timeout: Option<u64>,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            base_url: default_base_url(),
            model: default_model(),
            min_confidence: None,
            api_key: None,
            timeout: None,
        }
    }
}

/// The rone config file: named profiles plus an optional default profile.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Config {
    /// Name of the profile to use when `--profile` is not given.
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

impl Config {
    /// Parse a config from TOML content, validating confidence ranges and the
    /// existence of `default_profile`.
    pub fn parse(content: &str) -> Result<Config> {
        let config: Config = toml::from_str(content).context("failed to parse config TOML")?;
        for (name, profile) in &config.profiles {
            if let Some(min) = profile.min_confidence
                && !(0.0..=1.0).contains(&min)
            {
                bail!("profile '{name}': min_confidence {min} must be between 0 and 1");
            }
        }
        if let Some(default) = &config.default_profile
            && !config.profiles.contains_key(default)
        {
            bail!("default_profile '{default}' is not defined under [profiles.*]");
        }
        Ok(config)
    }

    /// Resolve the profile to use: `name` if given, else `default_profile`,
    /// else a built-in default profile.
    pub fn resolve_profile(&self, name: Option<&str>) -> Result<Profile> {
        let name = match name {
            Some(n) => n,
            None => match &self.default_profile {
                Some(d) => d.as_str(),
                None => return Ok(Profile::default()),
            },
        };
        let profile = self
            .profiles
            .get(name)
            .with_context(|| format!("unknown profile '{name}'"))?
            .clone();
        Ok(profile)
    }
}

/// Resolve an api_key value: a plain string is used as-is; a value of the
/// form `env:VAR` is replaced by the contents of the environment variable
/// `VAR` (an error if it is missing or empty).
pub fn resolve_api_key(raw: &str) -> Result<String> {
    match raw.strip_prefix("env:") {
        Some(var) => {
            let value = std::env::var(var)
                .with_context(|| format!("api_key references env var '{var}', which is not set"))?;
            if value.is_empty() {
                bail!("api_key references env var '{var}', which is empty");
            }
            Ok(value)
        }
        None => Ok(raw.to_string()),
    }
}

/// The profile named by the `RONE_PROFILE` environment variable, if set
/// and non-empty. Used as a fallback when `--profile` is not given.
pub fn profile_from_env() -> Option<String> {
    std::env::var("RONE_PROFILE").ok().filter(|s| !s.is_empty())
}

/// Displayable form of an api_key: `env:VAR` references are shown as-is,
/// literal values are hidden (shown as `set`) so listing profiles never
/// leaks a secret.
fn api_key_display(api_key: &Option<String>) -> Option<&str> {
    match api_key {
        Some(k) if k.starts_with("env:") => Some(k.as_str()),
        Some(_) => Some("set"),
        None => None,
    }
}

/// Render the profiles of a config as a human-friendly listing. The
/// default profile is marked with `*`; optional fields are only shown when
/// set.
pub fn render_profiles(config: &Config, colors: ColorMode) -> String {
    let mut out = String::new();
    for (name, profile) in &config.profiles {
        let is_default = config.default_profile.as_deref() == Some(name.as_str());
        let name = if is_default {
            colors.green(name)
        } else {
            colors.bold(name)
        };
        let marker = if is_default { "*" } else { " " };
        out.push_str(&format!("{marker} {name}\n"));
        out.push_str(&format!("    base_url        {}\n", profile.base_url));
        out.push_str(&format!("    model           {}\n", profile.model));
        if let Some(min) = profile.min_confidence {
            out.push_str(&format!("    min_confidence  {min}\n"));
        }
        if let Some(key) = api_key_display(&profile.api_key) {
            out.push_str(&format!("    api_key         {key}\n"));
        }
        if let Some(t) = profile.timeout {
            out.push_str(&format!("    timeout         {t}s\n"));
        }
    }
    out
}

/// Candidate config file locations, in order of precedence.
pub fn config_candidates(explicit: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = explicit {
        candidates.push(p.to_path_buf());
    }
    if let Ok(env) = std::env::var("RONE_CONFIG")
        && !env.is_empty()
    {
        candidates.push(PathBuf::from(env));
    }
    candidates.push(PathBuf::from("rone.toml"));
    let home_config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
        _ => std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".config"))
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    candidates.push(home_config.join("rone/rone.toml"));
    candidates
}

/// Load the config file to use. An explicit `--config` path must exist.
/// Otherwise the first existing candidate is used; `Ok(None)` when none is
/// found (built-in defaults apply).
pub fn load_config(explicit: Option<&Path>) -> Result<Option<Config>> {
    if let Some(p) = explicit {
        let content = std::fs::read_to_string(p)
            .with_context(|| format!("failed to read config file {}", p.display()))?;
        let config =
            Config::parse(&content).with_context(|| format!("in config file {}", p.display()))?;
        return Ok(Some(config));
    }
    for candidate in config_candidates(None) {
        if candidate.is_file() {
            let content = std::fs::read_to_string(&candidate)
                .with_context(|| format!("failed to read config file {}", candidate.display()))?;
            let config = Config::parse(&content)
                .with_context(|| format!("in config file {}", candidate.display()))?;
            return Ok(Some(config));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The environment is process-wide: tests that read or write it must be
    /// serialized, not run on parallel threads.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn parse_reads_profiles_and_default() {
        let cfg = Config::parse(
            r#"
default_profile = "local"

[profiles.local]
base_url = "http://localhost:1234/v1"
model = "laya"
min_confidence = 0.8

[profiles.prod]
base_url = "https://api.example.com/v1"
model = "laya-2"
"#,
        )
        .unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("local"));
        assert_eq!(cfg.profiles.len(), 2);
        let local = &cfg.profiles["local"];
        assert_eq!(local.base_url, "http://localhost:1234/v1");
        assert_eq!(local.model, "laya");
        assert_eq!(local.min_confidence, Some(0.8));
        let prod = &cfg.profiles["prod"];
        assert_eq!(prod.base_url, "https://api.example.com/v1");
        assert_eq!(prod.model, "laya-2");
        // A field left unset falls back to the built-in default (None here).
        assert_eq!(prod.min_confidence, None);
    }

    #[test]
    fn parse_applies_builtin_defaults_for_missing_fields() {
        let cfg = Config::parse("[profiles.a]").unwrap();
        let a = &cfg.profiles["a"];
        assert_eq!(a.base_url, DEFAULT_BASE_URL);
        assert_eq!(a.model, DEFAULT_MODEL);
        assert_eq!(a.min_confidence, None);
    }

    #[test]
    fn parse_rejects_out_of_range_confidence() {
        let err = Config::parse("[profiles.a]\nmin_confidence = 1.5").unwrap_err();
        assert!(err.to_string().contains("min_confidence"));
    }

    #[test]
    fn parse_rejects_negative_confidence() {
        let err = Config::parse("[profiles.a]\nmin_confidence = -0.1").unwrap_err();
        assert!(err.to_string().contains("min_confidence"));
    }

    #[test]
    fn parse_rejects_unknown_default_profile() {
        let err = Config::parse("default_profile = \"nope\"\n[profiles.a]").unwrap_err();
        assert!(err.to_string().contains("default_profile"));
    }

    #[test]
    fn parse_allows_empty_config() {
        let cfg = Config::parse("# nothing here").unwrap();
        assert!(cfg.profiles.is_empty());
        assert_eq!(cfg.default_profile, None);
    }

    #[test]
    fn resolve_profile_by_name() {
        let cfg = Config::parse("[profiles.a]\nmodel = \"m-a\"").unwrap();
        let p = cfg.resolve_profile(Some("a")).unwrap();
        assert_eq!(p.model, "m-a");
    }

    #[test]
    fn resolve_profile_falls_back_to_default_profile() {
        let cfg = Config::parse(
            "default_profile = \"a\"\n[profiles.a]\nmodel = \"m-a\"\n[profiles.b]\nmodel = \"m-b\"",
        )
        .unwrap();
        let p = cfg.resolve_profile(None).unwrap();
        assert_eq!(p.model, "m-a");
    }

    #[test]
    fn resolve_profile_none_gives_builtin_defaults() {
        let cfg = Config::parse("[profiles.a]\nmodel = \"m-a\"").unwrap();
        let p = cfg.resolve_profile(None).unwrap();
        assert_eq!(p.model, DEFAULT_MODEL);
        assert_eq!(p.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn resolve_profile_unknown_is_an_error() {
        let cfg = Config::parse("[profiles.a]").unwrap();
        assert!(cfg.resolve_profile(Some("zzz")).is_err());
    }

    #[test]
    fn candidates_put_explicit_first() {
        let explicit = Path::new("/tmp/some.rone.toml");
        let c = config_candidates(Some(explicit));
        assert_eq!(c[0], PathBuf::from("/tmp/some.rone.toml"));
    }

    #[test]
    fn candidates_without_explicit_start_with_cwd() {
        let c = config_candidates(None);
        assert_eq!(c[0], PathBuf::from("rone.toml"));
    }

    #[test]
    fn load_config_reads_explicit_file() {
        let path = std::env::temp_dir().join(format!("rone-cfg-{}.toml", std::process::id()));
        std::fs::write(&path, "[profiles.a]\nmodel = \"m-a\"").unwrap();
        let cfg = load_config(Some(&path))
            .unwrap()
            .expect("config should load");
        assert_eq!(cfg.profiles["a"].model, "m-a");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_config_explicit_missing_is_an_error() {
        let path =
            std::env::temp_dir().join(format!("rone-cfg-missing-{}.toml", std::process::id()));
        assert!(load_config(Some(&path)).is_err());
    }

    #[test]
    fn parse_reads_api_key_and_timeout() {
        let cfg = Config::parse(
            r#"[profiles.a]
api_key = "env:RONE_API_KEY"
timeout = 30
"#,
        )
        .unwrap();
        let a = &cfg.profiles["a"];
        // The raw value is kept as-is; env resolution happens at use time.
        assert_eq!(a.api_key.as_deref(), Some("env:RONE_API_KEY"));
        assert_eq!(a.timeout, Some(30));
    }

    #[test]
    fn parse_defaults_api_key_and_timeout_to_none() {
        let cfg = Config::parse("[profiles.a]").unwrap();
        let a = &cfg.profiles["a"];
        assert_eq!(a.api_key, None);
        assert_eq!(a.timeout, None);
    }

    #[test]
    fn resolve_api_key_returns_literal_as_is() {
        assert_eq!(resolve_api_key("sk-abc").unwrap(), "sk-abc");
    }

    #[test]
    fn resolve_api_key_reads_env_var() {
        unsafe { std::env::set_var("RONE_TEST_API_KEY", "from-env") };
        assert_eq!(
            resolve_api_key("env:RONE_TEST_API_KEY").unwrap(),
            "from-env"
        );
        unsafe { std::env::remove_var("RONE_TEST_API_KEY") };
    }

    #[test]
    fn resolve_api_key_env_missing_is_an_error() {
        let err = resolve_api_key("env:RONE_TEST_NOT_SET").unwrap_err();
        assert!(err.to_string().contains("RONE_TEST_NOT_SET"));
    }

    #[test]
    fn resolve_api_key_env_empty_is_an_error() {
        unsafe { std::env::set_var("RONE_TEST_EMPTY_KEY", "") };
        let err = resolve_api_key("env:RONE_TEST_EMPTY_KEY").unwrap_err();
        assert!(err.to_string().contains("empty"));
        unsafe { std::env::remove_var("RONE_TEST_EMPTY_KEY") };
    }

    #[test]
    fn render_profiles_lists_fields_and_default_marker() {
        let cfg = Config::parse(
            r#"default_profile = "a"

[profiles.a]
model = "m-a"
min_confidence = 0.8
api_key = "env:RONE_API_KEY"
timeout = 30

[profiles.b]
base_url = "https://api.example.com/v1"
api_key = "sk-literal"
"#,
        )
        .unwrap();
        let out = render_profiles(&cfg, ColorMode::off());
        let expected = [
            "* a",
            "    base_url        http://localhost:1234/v1",
            "    model           m-a",
            "    min_confidence  0.8",
            "    api_key         env:RONE_API_KEY",
            "    timeout         30s",
            "  b",
            "    base_url        https://api.example.com/v1",
            "    model           laya",
            "    api_key         set",
        ]
        .join("\n")
            + "\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn profile_from_env_reads_rone_profile() {
        let _env = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("RONE_PROFILE", "mini-ai") };
        assert_eq!(profile_from_env().as_deref(), Some("mini-ai"));
        unsafe { std::env::remove_var("RONE_PROFILE") };
    }

    #[test]
    fn profile_from_env_unset_or_empty_is_none() {
        let _env = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::remove_var("RONE_PROFILE") };
        assert_eq!(profile_from_env(), None);
        unsafe { std::env::set_var("RONE_PROFILE", "") };
        assert_eq!(profile_from_env(), None);
        unsafe { std::env::remove_var("RONE_PROFILE") };
    }

    #[test]
    fn render_profiles_off_mode_has_no_ansi() {
        let cfg = Config::parse("[profiles.a]").unwrap();
        assert!(!render_profiles(&cfg, ColorMode::off()).contains('\u{1b}'));
    }

    #[test]
    fn render_profiles_colored_mode_paints_ansi() {
        let cfg = Config::parse("default_profile = \"a\"\n[profiles.a]").unwrap();
        assert!(render_profiles(&cfg, ColorMode::on()).contains('\u{1b}'));
    }
}
