//! Plain ANSI colors for the human report of `alrunner test` (no dependency). `--color auto|always|never`:
//! auto colors only when the report goes to a terminal and `NO_COLOR` is not set. JUnit and JSON output are never colored.

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorMode {
    pub fn parse(v: &str) -> Option<Self> {
        match v {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    /// Whether to color. `is_terminal`: the stream the report goes to is a terminal. `no_color`: the `NO_COLOR`
    /// variable is set and not empty (https://no-color.org). `always` and `never` are explicit and win.
    pub fn enabled(self, is_terminal: bool, no_color: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => is_terminal && !no_color,
        }
    }
}

static ON: AtomicBool = AtomicBool::new(false);

pub fn set(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

fn paint(code: &str, text: &str) -> String {
    if ON.load(Ordering::Relaxed) {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn bold(t: &str) -> String {
    paint("1", t)
}
pub fn green(t: &str) -> String {
    paint("32", t)
}
pub fn red(t: &str) -> String {
    paint("31", t)
}
pub fn yellow(t: &str) -> String {
    paint("33", t)
}
pub fn dim(t: &str) -> String {
    paint("2", t)
}
pub fn cyan(t: &str) -> String {
    paint("36", t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_colors_only_on_a_terminal_without_no_color_and_explicit_modes_win() {
        assert!(ColorMode::Auto.enabled(true, false));
        assert!(!ColorMode::Auto.enabled(false, false));
        assert!(!ColorMode::Auto.enabled(true, true));
        assert!(ColorMode::Always.enabled(false, true));
        assert!(!ColorMode::Never.enabled(true, false));
        assert_eq!(ColorMode::parse("always"), Some(ColorMode::Always));
        assert_eq!(ColorMode::parse("sometimes"), None);
    }

    #[test]
    fn painting_wraps_in_codes_only_when_on() {
        set(false);
        assert_eq!(red("FAIL"), "FAIL");
        set(true);
        assert_eq!(red("FAIL"), "\x1b[31mFAIL\x1b[0m");
        set(false);
    }
}
