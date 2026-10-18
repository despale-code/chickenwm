//! `windowrulev2` matching and effects, shared by the WM (applies them) and `chickenctl
//! windowrules` (explains them).
//!
//! A rule is `effect, matcher[, matcher…]`. Every matcher must hit for the rule to apply.
//! Matchers: `class:`, `initialClass:`, `title:`, `initialTitle:`, `process:` (exe name), each a
//! case-insensitive regex (an invalid regex is matched as literal text). Effects: `float`,
//! `tile`, `fullscreen`, `pseudo`, `pin`, `center`, `workspace N`, `monitor N|name`,
//! `size W H`, `move X Y`, `opacity A`.

use crate::config::WindowRuleV2;
use regex::{Regex, RegexBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Class,
    Title,
    Process,
}

/// What a window is matched on.
pub struct WindowFacts<'a> {
    pub class: &'a str,
    pub title: &'a str,
    pub process: &'a str,
}

/// One parsed effect.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Float,
    Tile,
    Fullscreen,
    Pseudo,
    Pin,
    Center,
    Workspace(u32),
    Monitor(String),
    Size(i32, i32),
    Move(i32, i32),
    Opacity(f32),
}

pub struct CompiledRule {
    pub rule: WindowRuleV2,
    pub effect: Option<Effect>,
    matchers: Vec<(Field, Regex)>,
    /// Matcher keys that aren't understood: the rule never applies, and `problems` says why.
    unknown: Vec<String>,
}

impl CompiledRule {
    pub fn new(rule: &WindowRuleV2) -> Self {
        let mut matchers = Vec::new();
        let mut unknown = Vec::new();
        for part in rule.matcher.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let Some((key, pat)) = part.split_once(':') else {
                unknown.push(part.to_string());
                continue;
            };
            let field = match key.trim() {
                "class" | "initialClass" => Field::Class,
                "title" | "initialTitle" => Field::Title,
                "process" | "exe" => Field::Process,
                other => {
                    unknown.push(other.to_string());
                    continue;
                }
            };
            let pat = pat.trim();
            let re = RegexBuilder::new(pat)
                .case_insensitive(true)
                .build()
                .unwrap_or_else(|_| Regex::new(&format!("(?i){}", regex::escape(pat))).expect("escaped literal"));
            matchers.push((field, re));
        }
        Self { rule: rule.clone(), effect: parse_effect(&rule.effect), matchers, unknown }
    }

    pub fn matches(&self, w: &WindowFacts) -> bool {
        self.unknown.is_empty()
            && !self.matchers.is_empty()
            && self.matchers.iter().all(|(f, re)| {
                re.is_match(match f {
                    Field::Class => w.class,
                    Field::Title => w.title,
                    Field::Process => w.process,
                })
            })
    }

    /// Why this rule can never apply, if it can't.
    pub fn problems(&self) -> Vec<String> {
        let mut out: Vec<String> = self.unknown.iter().map(|k| format!("unknown matcher `{k}`")).collect();
        if self.matchers.is_empty() && self.unknown.is_empty() {
            out.push("no matcher".into());
        }
        if self.effect.is_none() {
            out.push(format!("unknown effect `{}`", self.rule.effect.trim()));
        }
        out
    }
}

pub fn compile(rules: &[WindowRuleV2]) -> Vec<CompiledRule> {
    rules.iter().map(CompiledRule::new).collect()
}

/// The effects that apply to a window, in rule order (later rules win where they conflict).
pub fn effects_for<'a>(rules: &'a [CompiledRule], w: &WindowFacts) -> Vec<&'a Effect> {
    rules.iter().filter(|r| r.matches(w)).filter_map(|r| r.effect.as_ref()).collect()
}

pub fn parse_effect(effect: &str) -> Option<Effect> {
    let lower = effect.trim().to_ascii_lowercase();
    let mut parts = lower.split_whitespace();
    let head = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    let num = |i: usize| rest.get(i).and_then(|s| s.parse::<i32>().ok());
    Some(match head {
        "float" | "floating" => Effect::Float,
        "tile" => Effect::Tile,
        "fullscreen" => Effect::Fullscreen,
        "pseudo" => Effect::Pseudo,
        "pin" => Effect::Pin,
        "center" => Effect::Center,
        "workspace" => Effect::Workspace(rest.first()?.parse::<u32>().ok()?.clamp(1, 10)),
        // Keep the original case: monitor names are matched as written.
        "monitor" => Effect::Monitor(effect.trim().split_whitespace().nth(1)?.to_string()),
        "size" => Effect::Size(num(0)?.max(100), num(1)?.max(80)),
        "move" => Effect::Move(num(0)?, num(1)?),
        "opacity" => Effect::Opacity(rest.first()?.parse::<f32>().ok()?.clamp(0.15, 1.0)),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(effect: &str, matcher: &str) -> CompiledRule {
        CompiledRule::new(&WindowRuleV2 { effect: effect.into(), matcher: matcher.into() })
    }

    const W: WindowFacts = WindowFacts { class: "Chrome_WidgetWin_1", title: "Discord", process: "Discord.exe" };

    #[test]
    fn regex_anchors_and_case() {
        assert!(rule("float", "title:^(discord)$").matches(&W));
        assert!(!rule("float", "title:^(Disc)$").matches(&W));
        assert!(rule("float", "class:chrome_widget").matches(&W));
    }

    #[test]
    fn all_matchers_must_hit() {
        assert!(rule("workspace 9", "class:Chrome, process:^discord\\.exe$").matches(&W));
        assert!(!rule("workspace 9", "class:Chrome, process:^slack").matches(&W));
    }

    #[test]
    fn invalid_regex_is_literal_and_unknown_keys_never_match() {
        assert!(!rule("float", "title:(unclosed").matches(&W));
        let r = rule("float", "wmclass:x");
        assert!(!r.matches(&W));
        assert_eq!(r.problems(), vec!["unknown matcher `wmclass`".to_string()]);
    }

    #[test]
    fn effects_parse() {
        assert_eq!(parse_effect("workspace 42"), Some(Effect::Workspace(10)));
        assert_eq!(parse_effect("size 10 10"), Some(Effect::Size(100, 80)));
        assert_eq!(parse_effect("move 20 -5"), Some(Effect::Move(20, -5)));
        assert_eq!(parse_effect("monitor DISPLAY2"), Some(Effect::Monitor("DISPLAY2".into())));
        assert_eq!(parse_effect("nofocus"), None);
        assert!(rule("nofocus", "class:x").problems().iter().any(|p| p.contains("unknown effect")));
    }
}
