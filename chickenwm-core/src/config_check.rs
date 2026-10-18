//! Checks `chickenwm.conf` and reports problems with their line numbers. The parser itself is
//! forgiving (unknown keys and bad values are skipped silently); this says which lines were
//! skipped and why, with a "did you mean" for typos.
//!
//! The known categories and keys are read from the parser's own source at compile time, so the
//! check never drifts from what `apply_category_kv` actually understands.

use crate::config::RgbaColor;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Issue {
    /// 1-based.
    pub line: usize,
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl Issue {
    /// Checks whether this issue targets `(category, key)` either directly or via a "did you mean" suggestion.
    pub fn targets_setting(&self, category: &str, key: &str) -> bool {
        let cat_ok = self
            .category
            .as_deref()
            .is_none_or(|c| category.is_empty() || c.eq_ignore_ascii_case(category));
        if !cat_ok {
            return false;
        }
        if self.key.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(key)) {
            return true;
        }
        let needle = format!("`{}`", key.to_ascii_lowercase());
        self.message.to_ascii_lowercase().contains(&needle)
    }
}

const PARSER_SOURCE: &str = include_str!("config.rs");

/// (categories, keys by category) understood by `apply_category_kv`.
fn known() -> &'static (HashSet<String>, HashMap<String, HashSet<String>>) {
    static KNOWN: std::sync::OnceLock<(HashSet<String>, HashMap<String, HashSet<String>>)> = std::sync::OnceLock::new();
    KNOWN.get_or_init(|| {
        let start = PARSER_SOURCE.find("pub fn apply_category_kv").unwrap_or(0);
        let end = PARSER_SOURCE[start..].find("pub fn apply_directive").map_or(PARSER_SOURCE.len(), |e| start + e);
        let body = &PARSER_SOURCE[start..end];
        let literals = |s: &str| -> Vec<String> { s.split('"').skip(1).step_by(2).map(str::to_owned).collect() };
        let mut cats = HashSet::new();
        let mut keys: HashMap<String, HashSet<String>> = HashMap::new();
        let mut current = String::new();
        for line in body.lines() {
            let indent = line.len() - line.trim_start().len();
            let t = line.trim();
            if t.starts_with('"') && t.contains("=>") {
                let pat = &t[..t.find("=>").unwrap_or(0)];
                if indent <= 12 {
                    for c in literals(pat) {
                        cats.insert(c.clone());
                        current = c;
                    }
                } else {
                    keys.entry(current.clone()).or_default().extend(literals(pat));
                }
            } else if let Some(i) = t.find("k == \"") {
                keys.entry(current.clone()).or_default().extend(literals(&t[i..]).into_iter().take(1));
            }
        }
        (cats, keys)
    })
}

fn top_level_directives() -> &'static [&'static str] {
    &[
        "monitor", "exec-once", "exec", "bezier", "animation", "bind", "bindr", "binde", "bindm", "bindl", "submap",
        "windowrule", "windowrulev2", "workspace", "source", "env", "layerrule", "wallpaper", "bindel", "bindle",
    ]
}

/// Plain Levenshtein distance, for "did you mean".
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn suggest<'a>(word: &str, candidates: impl Iterator<Item = &'a String>) -> Option<&'a String> {
    candidates
        .map(|c| (distance(word, c), c))
        .filter(|(d, c)| *d <= (c.len() / 3).max(2))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn is_bool(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "true" | "false" | "yes" | "no" | "on" | "off" | "1" | "0")
}

/// Keys whose value must be a boolean / number / colour, judged from the default config.
fn value_kind(key: &str) -> Option<&'static str> {
    static KINDS: std::sync::OnceLock<HashMap<String, &'static str>> = std::sync::OnceLock::new();
    let kinds = KINDS.get_or_init(|| {
        let mut m = HashMap::new();
        for line in crate::config::DEFAULT_CHICKENWM_CONF.lines() {
            let t = line.trim();
            if t.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = t.split_once('=') {
                let (k, v) = (k.trim().to_ascii_lowercase(), v.split(" #").next().unwrap_or("").trim().to_ascii_lowercase());
                let kind = if v == "true" || v == "false" {
                    Some("a true/false value")
                } else if v.parse::<f64>().is_ok() && !k.starts_with("col.") {
                    Some("a number")
                } else if k.starts_with("col.") {
                    Some("a colour")
                } else {
                    None
                };
                if let Some(kind) = kind {
                    m.insert(k, kind);
                }
            }
        }
        m
    });
    kinds.get(&key.to_ascii_lowercase()).copied()
}

fn value_ok(kind: &str, v: &str) -> bool {
    let v = v.trim();
    if v.contains('$') {
        return true; // a variable: checked separately
    }
    match kind {
        "a true/false value" => is_bool(v),
        "a number" => v.parse::<f64>().is_ok(),
        // Gradients: several colours and an optional angle.
        "a colour" => v.split_whitespace().all(|t| RgbaColor::parse(t).is_some() || t.ends_with("deg")),
        _ => true,
    }
}

/// Checks a whole `chickenwm.conf`. Issues come back in line order.
pub fn validate(source: &str) -> Vec<Issue> {
    let (cats, keys) = known();
    let mut issues = Vec::new();
    let mut stack: Vec<(String, usize)> = Vec::new();
    let mut vars: HashSet<String> = HashSet::new();
    let mut push_meta = |line: usize, severity: Severity, category: Option<String>, key: Option<String>, message: String| {
        issues.push(Issue {
            line,
            severity,
            message,
            category,
            key,
        })
    };

    for (i, raw) in source.lines().enumerate() {
        let n = i + 1;
        // Comments (a `#` at the start or after whitespace), but not colours like `#ffffff`.
        let mut t = raw.trim();
        if let Some(pos) = t.find(" #") {
            t = t[..pos].trim_end();
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "}" || t.starts_with("} ") {
            if stack.pop().is_none() {
                push_meta(n, Severity::Error, None, None, "`}` without a matching `{`".into());
            }
            continue;
        }
        if let Some(name) = t.strip_suffix('{').map(str::trim).filter(|s| !s.contains('=')) {
            let path = match stack.last() {
                Some((parent, _)) => format!("{parent}:{}", name.to_ascii_lowercase()),
                None => name.to_ascii_lowercase(),
            };
            if !cats.contains(&path) {
                let hint = suggest(&path, cats.iter()).map(|s| format!(" — did you mean `{s}`?")).unwrap_or_default();
                push_meta(
                    n,
                    Severity::Warning,
                    Some(path.clone()),
                    None,
                    format!("unknown section `{name}`; its settings are ignored{hint}"),
                );
            }
            stack.push((path, n));
            continue;
        }
        let Some((lhs, rhs)) = t.split_once('=') else {
            let cur_cat = stack.last().map(|(c, _)| c.clone());
            push_meta(n, Severity::Error, cur_cat, None, format!("not a setting (expected `name = value`): `{t}`"));
            continue;
        };
        let (key, value) = (lhs.trim(), rhs.trim());
        if let Some(var) = key.strip_prefix('$') {
            vars.insert(var.trim().to_owned());
            continue;
        }
        let cur_cat = stack.last().map(|(c, _)| c.clone());
        for used in value.split('$').skip(1) {
            let name: String = used.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            if !name.is_empty() && !vars.iter().any(|v| name.starts_with(v.as_str())) {
                push_meta(
                    n,
                    Severity::Warning,
                    cur_cat.clone(),
                    Some(key.to_ascii_lowercase()),
                    format!("`${name}` isn't defined (define it above with `${name} = …`)"),
                );
            }
        }
        match stack.last() {
            None => {
                if !top_level_directives().iter().any(|d| d.eq_ignore_ascii_case(key)) {
                    let all: Vec<String> = top_level_directives().iter().map(|s| s.to_string()).collect();
                    let hint = suggest(&key.to_ascii_lowercase(), all.iter()).map(|s| format!(" — did you mean `{s}`?")).unwrap_or_default();
                    push_meta(
                        n,
                        Severity::Warning,
                        None,
                        Some(key.to_ascii_lowercase()),
                        format!("`{key}` isn't a top-level setting; put it inside a section{hint}"),
                    );
                } else if key.eq_ignore_ascii_case("windowrulev2") || key.eq_ignore_ascii_case("windowrule") {
                    match value.split_once(',') {
                        None => push_meta(
                            n,
                            Severity::Error,
                            None,
                            Some("windowrulev2".into()),
                            "a window rule needs `effect, matcher`; it's ignored".into(),
                        ),
                        Some((effect, matcher)) => {
                            let rule = crate::config::WindowRuleV2 { effect: effect.trim().into(), matcher: matcher.trim().into() };
                            for p in crate::window_rules::CompiledRule::new(&rule).problems() {
                                push_meta(
                                    n,
                                    Severity::Warning,
                                    None,
                                    Some("windowrulev2".into()),
                                    format!("window rule: {p}; the rule never applies"),
                                );
                            }
                        }
                    }
                }
            }
            Some((cat, _)) => {
                if !cats.contains(cat) {
                    continue; // already reported for the section
                }
                let k = key.to_ascii_lowercase();
                let cat_keys = keys.get(cat);
                if !cat_keys.is_some_and(|s| s.contains(&k)) && !top_level_directives().contains(&k.as_str()) {
                    let suggested = cat_keys.and_then(|s| suggest(&k, s.iter())).cloned();
                    let hint = suggested.as_ref().map(|s| format!(" — did you mean `{s}`?")).unwrap_or_default();
                    push_meta(
                        n,
                        Severity::Warning,
                        Some(cat.clone()),
                        suggested.or(Some(k)),
                        format!("`{key}` isn't a `{cat}` setting; it's ignored{hint}"),
                    );
                } else if let Some(kind) = value_kind(&k) {
                    if !value_ok(kind, value) {
                        push_meta(
                            n,
                            Severity::Error,
                            Some(cat.clone()),
                            Some(k),
                            format!("`{key}` needs {kind}, got `{value}`; it's ignored"),
                        );
                    }
                }
            }
        }
    }
    for (cat, line) in stack {
        issues.push(Issue {
            line,
            severity: Severity::Error,
            message: format!("section `{cat}` is never closed (missing `}}`)"),
            category: Some(cat),
            key: None,
        });
    }
    issues.sort_by_key(|i| i.line);
    issues
}

/// Checks `~/.config/chickenwm/chickenwm.conf`.
pub fn validate_user_config() -> anyhow::Result<Vec<Issue>> {
    let text = std::fs::read_to_string(crate::config::config_dir().join("chickenwm.conf"))?;
    Ok(validate(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_clean() {
        let issues = validate(crate::config::DEFAULT_CHICKENWM_CONF);
        assert!(issues.is_empty(), "{issues:#?}");
    }

    #[test]
    fn finds_mistakes() {
        let src = "general {\n    gaps_inn = 5\n    border_size = thick\n    col.active_border = rgba(33ccffee) 45deg\n}\ndecortion {\n}\n}\ninput {\n    kb_layout = us\n    nonsense line\n$mod = SUPER\nbind = $mod, Q, killactive\nbind = $mood, Q, exit\n";
        let issues = validate(src);
        let msgs: Vec<(usize, &str)> = issues.iter().map(|i| (i.line, i.message.as_str())).collect();
        assert!(msgs.iter().any(|(l, m)| *l == 2 && m.contains("did you mean `gaps_in`")), "{msgs:#?}");
        assert!(msgs.iter().any(|(l, m)| *l == 3 && m.contains("a number")), "{msgs:#?}");
        assert!(!msgs.iter().any(|(l, _)| *l == 4), "{msgs:#?}");
        assert!(msgs.iter().any(|(l, m)| *l == 6 && m.contains("did you mean `decoration`")), "{msgs:#?}");
        assert!(msgs.iter().any(|(l, m)| *l == 8 && m.contains("without a matching")), "{msgs:#?}");
        assert!(msgs.iter().any(|(l, m)| *l == 11 && m.contains("not a setting")), "{msgs:#?}");
        assert!(msgs.iter().any(|(l, m)| *l == 14 && m.contains("$mood")), "{msgs:#?}");
        assert!(msgs.iter().any(|(l, m)| *l == 9 && m.contains("never closed")), "{msgs:#?}");
    }
}
