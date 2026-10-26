//! Terminal colors for human-friendly output.
//!
//! ANSI escape codes, no dependencies. All rendering goes through
//! [`ColorMode`], so when it is disabled the output stays byte-identical
//! to the plain version (pipes, tests, JSON mode).

/// A switch for colored output: when disabled, every paint is a no-op.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorMode {
    enabled: bool,
}

impl ColorMode {
    pub fn new(enabled: bool) -> Self {
        ColorMode { enabled }
    }

    pub const fn off() -> Self {
        ColorMode { enabled: false }
    }

    pub const fn on() -> Self {
        ColorMode { enabled: true }
    }

    pub const fn enabled(self) -> bool {
        self.enabled
    }

    /// Wrap `text` in an ANSI SGR escape; a no-op when disabled.
    fn paint(self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    pub fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    pub fn yellow(self, text: &str) -> String {
        self.paint("33", text)
    }

    pub fn red(self, text: &str) -> String {
        self.paint("31", text)
    }

    /// Paint a magnitude in `[0, 1]`: green below 0.5, yellow below 0.75,
    /// red above.
    pub fn magnitude(self, value: &str, ratio: f64) -> String {
        if ratio >= 0.75 {
            self.red(value)
        } else if ratio >= 0.5 {
            self.yellow(value)
        } else {
            self.green(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_mode_is_a_noop() {
        let c = ColorMode::off();
        assert_eq!(c.bold("x"), "x");
        assert_eq!(c.green("x"), "x");
        assert_eq!(c.yellow("x"), "x");
        assert_eq!(c.red("x"), "x");
        assert_eq!(c.magnitude("0.76", 0.76), "0.76");
    }

    #[test]
    fn on_mode_wraps_in_ansi_escapes() {
        let c = ColorMode::on();
        assert_eq!(c.bold("x"), "\u{1b}[1mx\u{1b}[0m");
        assert_eq!(c.green("x"), "\u{1b}[32mx\u{1b}[0m");
        assert_eq!(c.yellow("x"), "\u{1b}[33mx\u{1b}[0m");
        assert_eq!(c.red("x"), "\u{1b}[31mx\u{1b}[0m");
    }

    #[test]
    fn magnitude_picks_color_by_threshold() {
        let c = ColorMode::on();
        assert!(c.magnitude("0.4", 0.4).contains("[32m"));
        assert!(c.magnitude("0.6", 0.6).contains("[33m"));
        assert!(c.magnitude("0.8", 0.8).contains("[31m"));
        // Boundaries: 0.5 is yellow, 0.75 is red.
        assert!(c.magnitude("0.5", 0.5).contains("[33m"));
        assert!(c.magnitude("0.75", 0.75).contains("[31m"));
    }
}
