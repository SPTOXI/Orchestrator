//! Small text helpers shared by the Context Builder and the handoff.

use chrono::{DateTime, Utc};
use std::path::{Component, Path};

/// Rough token estimate: ≈ 4 characters per token (ADR-0013). The real
/// count comes from the provider after the turn.
pub fn estimate_tokens(text: &str) -> u32 {
    (text.chars().count() as u32).div_ceil(4)
}

/// `text` with every run of whitespace (newlines included) as one space.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` characters, with `…` when cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// One line of at most `max` characters.
pub fn line(text: &str, max: usize) -> String {
    clip(&one_line(text), max)
}

/// `path` relative to `root` when inside it, `/`-separated.
pub fn relative(path: &str, root: &Path) -> String {
    match Path::new(path).strip_prefix(root) {
        Ok(rest) if !rest.as_os_str().is_empty() => rest
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        _ => path.to_owned(),
    }
}

/// A relative path that stays inside its root (no `..`, not absolute).
pub fn is_inner_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// `2026-09-29 14:03 UTC`.
pub fn when(at: &DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("ação"), 1);
        assert_eq!(one_line(" a\n\n b\tc "), "a b c");
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc", 4), "abc");
        assert_eq!(line("uma\nduas  três", 9), "uma duas…");
        assert_eq!(
            relative("/p/saas/src/pay.ts", Path::new("/p/saas")),
            "src/pay.ts"
        );
        assert_eq!(relative("/outro/x.ts", Path::new("/p/saas")), "/outro/x.ts");
        assert!(is_inner_relative(Path::new("src/pay.ts")));
        assert!(!is_inner_relative(Path::new("../x")));
        assert!(!is_inner_relative(Path::new("/etc/passwd")));
    }
}
