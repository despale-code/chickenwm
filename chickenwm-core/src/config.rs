//! Phase 2.1, 2.2 & 10.3: `chickenwm.conf` Lexer, Parser (`winnow`), Lossless AST Serializer (2-Way Sync),
//! and Default Configuration Generator.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use winnow::ascii::{space0, till_line_ending};
use winnow::combinator::opt;
use winnow::prelude::*;
use winnow::token::take_while;

/// Returns the user's `~/.config/chickenwm` directory path.
pub fn config_dir() -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        home.join(".config").join("chickenwm")
    } else {
        PathBuf::from(".config/chickenwm")
    }
}

/// Returns the path to `~/.config/chickenwm/chickenwm.conf`.
pub fn config_file_path() -> PathBuf {
    config_dir().join("chickenwm.conf")
}

/// Lossless AST node representing a line or block in `chickenwm.conf`.
/// Preserves user comments (`#`), blank lines, indentation, and `$variable` references.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AstNode {
    /// Blank/whitespace-only line.
    Blank { raw: String },
    /// Standalone `# ...` comment line.
    Comment { raw: String },
    /// Variable definition (`$mainMod = SUPER`).
    Variable {
        indent: String,
        name: String,
        raw_value: String,
        trailing_comment: Option<String>,
    },
    /// Top-level directive (`bind = ...`, `monitor = ...`, `exec-once = ...`, `windowrulev2 = ...`, `bezier = ...`, `animation = ...`).
    Directive {
        indent: String,
        keyword: String,
        raw_value: String,
        trailing_comment: Option<String>,
    },
    /// Key-value assignment inside a category block (`gaps_in = 5`).
    KeyValue {
        indent: String,
        key: String,
        raw_value: String,
        trailing_comment: Option<String>,
    },
    /// Nested category block (`general { ... }`, `misc { ... }`, etc.).
    Category {
        indent: String,
        name: String,
        header_comment: Option<String>,
        children: Vec<AstNode>,
        footer_indent: String,
        footer_comment: Option<String>,
    },
}

/// Lossless syntax tree of `chickenwm.conf` supporting 2-way editing without formatting destruction.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfigDocument {
    pub nodes: Vec<AstNode>,
}

/// Parsed line representation produced by our `winnow` line lexer.
#[derive(Debug, Clone)]
enum ParsedLine {
    Blank(String),
    Comment(String),
    CategoryStart {
        indent: String,
        name: String,
        comment: Option<String>,
    },
    CategoryEnd {
        indent: String,
        comment: Option<String>,
    },
    Assignment {
        indent: String,
        lhs: String,
        rhs: String,
        comment: Option<String>,
    },
}

/// Splits a raw value into `(value_part, optional_trailing_comment)` while respecting quotes and `rgba(...)` / `0x...` hex literals.
fn split_value_and_comment(raw: &str) -> (String, Option<String>) {
    let mut in_quotes = false;
    let bytes = raw.as_bytes();
    for i in 0..bytes.len() {
        match bytes[i] {
            b'"' => in_quotes = !in_quotes,
            b'#' if !in_quotes => {
                // Treat `#` as a comment if it's at start or preceded by whitespace
                if i == 0 || bytes[i - 1].is_ascii_whitespace() {
                    let val = raw[..i].trim().to_string();
                    let comment = raw[i..].to_string();
                    return (val, Some(comment));
                }
            }
            _ => {}
        }
    }
    (raw.trim().to_string(), None)
}

/// `winnow` parser for a single line of `chickenwm.conf`.
fn parse_hypr_line(input: &mut &str) -> winnow::ModalResult<ParsedLine> {
    let indent: &str = space0.parse_next(input)?;
    let rest: &str = till_line_ending.parse_next(input)?;
    let trimmed = rest.trim();

    if trimmed.is_empty() {
        return Ok(ParsedLine::Blank(format!("{indent}{rest}")));
    }
    if trimmed.starts_with('#') {
        return Ok(ParsedLine::Comment(format!("{indent}{rest}")));
    }
    if let Some(after_brace) = trimmed.strip_prefix('}') {
        let after = after_brace.trim();
        let comment = if after.starts_with('#') {
            Some(after.to_string())
        } else {
            None
        };
        return Ok(ParsedLine::CategoryEnd {
            indent: indent.to_string(),
            comment,
        });
    }

    // Check if this is `category_name {` using a winnow sub-parser
    let mut sub = trimmed;
    let ident_res: winnow::ModalResult<&str> = take_while(1.., |c: char| {
        c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':' || c == '.'
    })
    .parse_next(&mut sub);

    if let Ok(ident) = ident_res {
        let _ = space0::<_, winnow::error::ContextError>.parse_next(&mut sub);
        let brace: Option<char> =
            opt::<_, _, winnow::error::ContextError, _>('{')
                .parse_next(&mut sub)
                .unwrap_or(None);
        if brace.is_some() {
            let rem = sub.trim();
            let comment = if rem.starts_with('#') {
                Some(rem.to_string())
            } else {
                None
            };
            return Ok(ParsedLine::CategoryStart {
                indent: indent.to_string(),
                name: ident.to_string(),
                comment,
            });
        }
    }

    // Check for `lhs = rhs` assignment / directive / variable
    if let Some(eq_idx) = trimmed.find('=') {
        let lhs = trimmed[..eq_idx].trim().to_string();
        let rhs_raw = trimmed[eq_idx + 1..].trim();
        let (rhs, comment) = split_value_and_comment(rhs_raw);
        return Ok(ParsedLine::Assignment {
            indent: indent.to_string(),
            lhs,
            rhs,
            comment,
        });
    }

    // Fallback: treat unparseable line as comment so lossless serializer never drops user text
    Ok(ParsedLine::Comment(format!("{indent}{rest}")))
}

fn is_top_level_directive(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "monitor"
            | "exec-once"
            | "exec"
            | "bezier"
            | "animation"
            | "bind"
            | "bindr"
            | "binde"
            | "bindm"
            | "bindl"
            | "windowrule"
            | "windowrulev2"
            | "wallpaper"
            | "env"
            | "source"
    )
}

impl ConfigDocument {
    /// Parses a `chickenwm.conf` string into a lossless `ConfigDocument` AST using `winnow`.
    pub fn parse(source: &str) -> Result<Self> {
        let mut root_nodes = Vec::new();
        let mut category_stack: Vec<(String, String, Option<String>, Vec<AstNode>)> = Vec::new();

        for raw_line in source.lines() {
            let mut slice = raw_line;
            let parsed = parse_hypr_line(&mut slice)
                .map_err(|e| anyhow::anyhow!("Failed to parse config line '{raw_line}': {e}"))?;

            let in_root_scope = category_stack.is_empty();
            let target_vec = if let Some((_, _, _, children)) = category_stack.last_mut() {
                children
            } else {
                &mut root_nodes
            };

            match parsed {
                ParsedLine::Blank(raw) => target_vec.push(AstNode::Blank { raw }),
                ParsedLine::Comment(raw) => target_vec.push(AstNode::Comment { raw }),
                ParsedLine::CategoryStart {
                    indent,
                    name,
                    comment,
                } => {
                    category_stack.push((indent, name, comment, Vec::new()));
                }
                ParsedLine::CategoryEnd { indent, comment } => {
                    if let Some((cat_indent, cat_name, header_comment, children)) =
                        category_stack.pop()
                    {
                        let parent_vec =
                            if let Some((_, _, _, parent_children)) = category_stack.last_mut() {
                                parent_children
                            } else {
                                &mut root_nodes
                            };
                        parent_vec.push(AstNode::Category {
                            indent: cat_indent,
                            name: cat_name,
                            header_comment,
                            children,
                            footer_indent: indent,
                            footer_comment: comment,
                        });
                    }
                }
                ParsedLine::Assignment {
                    indent,
                    lhs,
                    rhs,
                    comment,
                } => {
                    if let Some(var_name) = lhs.strip_prefix('$') {
                        target_vec.push(AstNode::Variable {
                            indent,
                            name: var_name.trim().to_string(),
                            raw_value: rhs,
                            trailing_comment: comment,
                        });
                    } else if in_root_scope && is_top_level_directive(&lhs) {
                        target_vec.push(AstNode::Directive {
                            indent,
                            keyword: lhs,
                            raw_value: rhs,
                            trailing_comment: comment,
                        });
                    } else {
                        target_vec.push(AstNode::KeyValue {
                            indent,
                            key: lhs,
                            raw_value: rhs,
                            trailing_comment: comment,
                        });
                    }
                }
            }
        }

        // Close any unclosed categories gracefully
        while let Some((cat_indent, cat_name, header_comment, children)) = category_stack.pop() {
            let parent_vec = if let Some((_, _, _, parent_children)) = category_stack.last_mut() {
                parent_children
            } else {
                &mut root_nodes
            };
            parent_vec.push(AstNode::Category {
                indent: cat_indent,
                name: cat_name,
                header_comment,
                children,
                footer_indent: String::new(),
                footer_comment: None,
            });
        }

        Ok(Self { nodes: root_nodes })
    }

    /// Serializes the AST back to `chickenwm.conf` format, preserving all comments, blank lines,
    /// indentation, and `$variable` references.
    pub fn to_lossless_string(&self) -> String {
        let mut out = String::new();
        Self::serialize_nodes(&self.nodes, &mut out);
        out
    }

    fn serialize_nodes(nodes: &[AstNode], out: &mut String) {
        for node in nodes {
            match node {
                AstNode::Blank { raw } => {
                    out.push_str(raw);
                    out.push('\n');
                }
                AstNode::Comment { raw } => {
                    out.push_str(raw);
                    out.push('\n');
                }
                AstNode::Variable {
                    indent,
                    name,
                    raw_value,
                    trailing_comment,
                } => {
                    out.push_str(indent);
                    out.push('$');
                    out.push_str(name);
                    out.push_str(" = ");
                    out.push_str(raw_value);
                    if let Some(c) = trailing_comment {
                        out.push(' ');
                        out.push_str(c);
                    }
                    out.push('\n');
                }
                AstNode::Directive {
                    indent,
                    keyword,
                    raw_value,
                    trailing_comment,
                } => {
                    out.push_str(indent);
                    out.push_str(keyword);
                    out.push_str(" = ");
                    out.push_str(raw_value);
                    if let Some(c) = trailing_comment {
                        out.push(' ');
                        out.push_str(c);
                    }
                    out.push('\n');
                }
                AstNode::KeyValue {
                    indent,
                    key,
                    raw_value,
                    trailing_comment,
                } => {
                    out.push_str(indent);
                    out.push_str(key);
                    out.push_str(" = ");
                    out.push_str(raw_value);
                    if let Some(c) = trailing_comment {
                        out.push(' ');
                        out.push_str(c);
                    }
                    out.push('\n');
                }
                AstNode::Category {
                    indent,
                    name,
                    header_comment,
                    children,
                    footer_indent,
                    footer_comment,
                } => {
                    out.push_str(indent);
                    out.push_str(name);
                    out.push_str(" {");
                    if let Some(hc) = header_comment {
                        out.push(' ');
                        out.push_str(hc);
                    }
                    out.push('\n');
                    Self::serialize_nodes(children, out);
                    out.push_str(footer_indent);
                    out.push('}');
                    if let Some(fc) = footer_comment {
                        out.push(' ');
                        out.push_str(fc);
                    }
                    out.push('\n');
                }
            }
        }
    }

    /// Updates a key inside a category (`general`, `decoration`, `misc`, etc.) in-place,
    /// preserving any trailing comment on that line, or creates the key/category if absent.
    pub fn set_category_key(&mut self, category: &str, key: &str, value: &str) {
        for node in &mut self.nodes {
            if let AstNode::Category { name, children, .. } = node {
                if name.eq_ignore_ascii_case(category) {
                    for child in children.iter_mut() {
                        if let AstNode::KeyValue {
                            key: k, raw_value, ..
                        } = child
                        {
                            if k.eq_ignore_ascii_case(key) {
                                *raw_value = value.to_string();
                                return;
                            }
                        }
                    }
                    children.push(AstNode::KeyValue {
                        indent: "    ".to_string(),
                        key: key.to_string(),
                        raw_value: value.to_string(),
                        trailing_comment: None,
                    });
                    return;
                }
            }
        }

        // Category did not exist yet; create it
        self.nodes.push(AstNode::Blank { raw: String::new() });
        self.nodes.push(AstNode::Category {
            indent: String::new(),
            name: category.to_string(),
            header_comment: None,
            children: vec![AstNode::KeyValue {
                indent: "    ".to_string(),
                key: key.to_string(),
                raw_value: value.to_string(),
                trailing_comment: None,
            }],
            footer_indent: String::new(),
            footer_comment: None,
        });
    }

    /// Updates or adds a `$variable = value` in the AST.
    pub fn set_variable(&mut self, var_name: &str, value: &str) {
        let clean = var_name.trim_start_matches('$');
        for node in &mut self.nodes {
            if let AstNode::Variable {
                name, raw_value, ..
            } = node
            {
                if name == clean {
                    *raw_value = value.to_string();
                    return;
                }
            }
        }
        self.nodes.insert(
            0,
            AstNode::Variable {
                indent: String::new(),
                name: clean.to_string(),
                raw_value: value.to_string(),
                trailing_comment: None,
            },
        );
    }

    /// Replaces all top-level directives matching `keyword` (e.g. `exec-once`, `windowrulev2`)
    /// with the new list of raw values, or appends a single directive.
    pub fn add_directive(&mut self, keyword: &str, value: &str) {
        self.nodes.push(AstNode::Directive {
            indent: String::new(),
            keyword: keyword.to_string(),
            raw_value: value.to_string(),
            trailing_comment: None,
        });
    }

    pub fn replace_directives(&mut self, keyword: &str, values: &[String]) {
        self.nodes.retain(|n| {
            !matches!(
                n,
                AstNode::Directive { keyword: k, .. } if k.eq_ignore_ascii_case(keyword)
            )
        });
        for v in values {
            self.add_directive(keyword, v);
        }
    }

    /// Compiles this `ConfigDocument` into a strongly-typed `HyprConfig` with all `$variables` expanded.
    pub fn compile(&self) -> HyprConfig {
        let mut cfg = HyprConfig::default();
        Self::eval_nodes(&self.nodes, None, &mut cfg);
        cfg
    }

    fn expand_vars(input: &str, vars: &HashMap<String, String>) -> String {
        let mut result = input.to_string();
        // Sort variable names longest-first so `$mainModShift` expands before `$mainMod`
        let mut keys: Vec<&String> = vars.keys().collect();
        keys.sort_by(|a, b| b.len().cmp(&a.len()));
        for k in keys {
            let token = format!("${k}");
            if result.contains(&token) {
                if let Some(val) = vars.get(k) {
                    result = result.replace(&token, val);
                }
            }
        }
        result
    }

    fn eval_nodes(nodes: &[AstNode], category: Option<&str>, cfg: &mut HyprConfig) {
        for node in nodes {
            match node {
                AstNode::Variable {
                    name, raw_value, ..
                } => {
                    let expanded = Self::expand_vars(raw_value, &cfg.variables);
                    cfg.variables.insert(name.clone(), expanded);
                }
                AstNode::Directive {
                    keyword, raw_value, ..
                } => {
                    let val = Self::expand_vars(raw_value, &cfg.variables);
                    cfg.apply_directive(keyword, &val);
                }
                AstNode::KeyValue { key, raw_value, .. } => {
                    let val = Self::expand_vars(raw_value, &cfg.variables);
                    if let Some(cat) = category {
                        cfg.apply_category_kv(cat, key, &val);
                    } else if is_top_level_directive(key) {
                        cfg.apply_directive(key, &val);
                    }
                }
                AstNode::Category { name, children, .. } => {
                    let cat_path = match category {
                        Some(parent) => format!("{parent}:{name}"),
                        None => name.to_ascii_lowercase(),
                    };
                    Self::eval_nodes(children, Some(&cat_path), cfg);
                }
                AstNode::Blank { .. } | AstNode::Comment { .. } => {}
            }
        }
    }
}

/// RGBA color parsed from `rgba(rrggbbaa)`, `rgb(rrggbb)`, `0xaarrggbb`, or `#rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Default for RgbaColor {
    fn default() -> Self {
        Self {
            r: 0x33,
            g: 0xcc,
            b: 0xff,
            a: 0xee,
        }
    }
}

impl RgbaColor {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(inner) = s.strip_prefix("rgba(").and_then(|v| v.strip_suffix(')')) {
            let inner = inner.trim();
            if inner.contains(',') {
                let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
                if parts.len() == 4 {
                    let r = parts[0].parse::<u8>().ok()?;
                    let g = parts[1].parse::<u8>().ok()?;
                    let b = parts[2].parse::<u8>().ok()?;
                    let a_f = parts[3].parse::<f32>().ok()?;
                    let a = if a_f <= 1.0 {
                        (a_f.clamp(0.0, 1.0) * 255.0).round() as u8
                    } else {
                        a_f.clamp(0.0, 255.0) as u8
                    };
                    return Some(Self { r, g, b, a });
                }
            } else if inner.len() == 8 {
                let hex = u32::from_str_radix(inner, 16).ok()?;
                return Some(Self {
                    r: ((hex >> 24) & 0xFF) as u8,
                    g: ((hex >> 16) & 0xFF) as u8,
                    b: ((hex >> 8) & 0xFF) as u8,
                    a: (hex & 0xFF) as u8,
                });
            }
        } else if let Some(inner) = s.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
            let inner = inner.trim();
            if inner.len() == 6 {
                let hex = u32::from_str_radix(inner, 16).ok()?;
                return Some(Self {
                    r: ((hex >> 16) & 0xFF) as u8,
                    g: ((hex >> 8) & 0xFF) as u8,
                    b: (hex & 0xFF) as u8,
                    a: 255,
                });
            }
        } else if let Some(hex_str) = s.strip_prefix("0x").or_else(|| s.strip_prefix('#')) {
            if hex_str.len() == 8 {
                let hex = u32::from_str_radix(hex_str, 16).ok()?;
                return Some(Self {
                    a: ((hex >> 24) & 0xFF) as u8,
                    r: ((hex >> 16) & 0xFF) as u8,
                    g: ((hex >> 8) & 0xFF) as u8,
                    b: (hex & 0xFF) as u8,
                });
            } else if hex_str.len() == 6 {
                let hex = u32::from_str_radix(hex_str, 16).ok()?;
                return Some(Self {
                    r: ((hex >> 16) & 0xFF) as u8,
                    g: ((hex >> 8) & 0xFF) as u8,
                    b: (hex & 0xFF) as u8,
                    a: 255,
                });
            }
        }
        None
    }

    pub fn to_hypr_rgba_string(&self) -> String {
        format!(
            "rgba({:02x}{:02x}{:02x}{:02x})",
            self.r, self.g, self.b, self.a
        )
    }

    pub fn to_normalized_f32(&self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        ]
    }
}

/// Border gradient specification (`rgba(33ccffee) rgba(00ff99ee) 45deg`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BorderColorSpec {
    pub primary: RgbaColor,
    pub secondary: Option<RgbaColor>,
    pub angle_deg: f32,
}

impl Default for BorderColorSpec {
    fn default() -> Self {
        Self {
            primary: RgbaColor {
                r: 0x33,
                g: 0xcc,
                b: 0xff,
                a: 0xee,
            },
            secondary: Some(RgbaColor {
                r: 0x00,
                g: 0xff,
                b: 0x99,
                a: 0xee,
            }),
            angle_deg: 45.0,
        }
    }
}

impl BorderColorSpec {
    pub fn parse(raw: &str) -> Self {
        let mut colors = Vec::new();
        let mut angle_deg = 45.0;
        for token in raw.split_whitespace() {
            if let Some(deg_str) = token.strip_suffix("deg") {
                if let Ok(d) = deg_str.parse::<f32>() {
                    angle_deg = d;
                }
            } else if let Some(c) = RgbaColor::parse(token) {
                colors.push(c);
            }
        }
        let primary = colors.first().copied().unwrap_or_default();
        let secondary = colors.get(1).copied();
        Self {
            primary,
            secondary,
            angle_deg,
        }
    }

    pub fn to_hypr_string(&self) -> String {
        match self.secondary {
            Some(sec) => format!(
                "{} {} {:.0}deg",
                self.primary.to_hypr_rgba_string(),
                sec.to_hypr_rgba_string(),
                self.angle_deg
            ),
            None => self.primary.to_hypr_rgba_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub gaps_in: i32,
    pub gaps_out: i32,
    pub border_size: i32,
    pub col_active_border: BorderColorSpec,
    pub col_inactive_border: BorderColorSpec,
    pub layout: String, // "dwindle" | "master"
    pub allow_tearing: bool,
    pub resize_on_border: bool,
    pub no_titlebars: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            gaps_in: 5,
            gaps_out: 10,
            border_size: 2,
            col_active_border: BorderColorSpec::default(),
            col_inactive_border: BorderColorSpec {
                primary: RgbaColor {
                    r: 0x59,
                    g: 0x59,
                    b: 0x59,
                    a: 0xaa,
                },
                secondary: None,
                angle_deg: 0.0,
            },
            layout: "dwindle".to_string(),
            allow_tearing: false,
            resize_on_border: true,
            no_titlebars: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecorationConfig {
    pub rounding: i32,
    pub active_opacity: f32,
    pub inactive_opacity: f32,
    pub fullscreen_opacity: f32,
    pub blur_enabled: bool,
    pub blur_size: i32,
    pub blur_passes: i32,
    pub blur_style: String, // "acrylic" | "mica" | "blur"
    pub drop_shadow: bool,
    pub border_anim_speed: f32,
}

impl Default for DecorationConfig {
    fn default() -> Self {
        Self {
            rounding: 10,
            active_opacity: 1.0,
            inactive_opacity: 0.92,
            fullscreen_opacity: 1.0,
            blur_enabled: true,
            blur_size: 8,
            blur_passes: 2,
            blur_style: "acrylic".to_string(),
            drop_shadow: true,
            border_anim_speed: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimationsConfig {
    pub enabled: bool,
    pub first_launch_animation: bool,
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            first_launch_animation: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DwindleConfig {
    pub pseudotile: bool,
    pub preserve_split: bool,
    pub force_split: u8, // 0 = follow aspect ratio, 1 = left/top, 2 = right/bottom
    pub default_split_ratio: f32,
    pub smart_gaps: bool,
}

impl Default for DwindleConfig {
    fn default() -> Self {
        Self {
            pseudotile: true,
            preserve_split: true,
            force_split: 0,
            default_split_ratio: 1.0,
            smart_gaps: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MasterConfig {
    pub mfact: f32,
    pub orientation: String, // "left", "right", "top", "bottom", "center"
    pub new_status: String,  // "master", "slave"
    pub smart_gaps: bool,
}

impl Default for MasterConfig {
    fn default() -> Self {
        Self {
            mfact: 0.55,
            orientation: "left".to_string(),
            new_status: "slave".to_string(),
            smart_gaps: false,
        }
    }
}

/// Phase 1.1, Phase 2.1 & Phase 3.2: Frame Pacing, Anti-Cheat Safe Switch & Misc Configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MiscConfig {
    /// Variable Frame Rate — suspend high-frequency tick loop when no animations are active.
    pub vfr: bool,
    /// `0` = follow active monitor's native refresh rate dynamically; `>0` = lock animations/resizes/renders to custom FPS target.
    pub fps_limit: u32,
    pub suppress_explorer: bool,
    pub replace_winlogon_shell: bool,
    pub register_files_handler: bool,
    /// Start ChickenWM with administrator rights (via a one-time-approved scheduled task) so
    /// windows of elevated programs can be tiled.
    #[serde(default)]
    pub run_as_admin: bool,
    /// Phase 1.1 & 9.3: Anti-Cheat Safe Switch (`false` = Standard Win32/DirectComposition mode; `true` = Experimental `chickenwm-dwm-hook` PDB injection).
    pub dwm_hook_enabled: bool,
    /// Window classes resized with a GPU-stretched preview during mouse drags (one real resize
    /// on release) instead of live resizing — for apps whose renderer can't keep up (Firefox/Zen).
    #[serde(default = "default_resize_preview_classes")]
    pub resize_preview_classes: Vec<String>,
}

fn default_resize_preview_classes() -> Vec<String> {
    vec!["MozillaWindowClass".to_string()]
}

impl Default for MiscConfig {
    fn default() -> Self {
        Self {
            vfr: true,
            fps_limit: 0, // 0 = follow monitor native refresh rate dynamically
            suppress_explorer: true,
            replace_winlogon_shell: false,
            register_files_handler: false,
            run_as_admin: false,
            dwm_hook_enabled: false,
            resize_preview_classes: default_resize_preview_classes(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputConfig {
    pub kb_layout: String,
    pub follow_mouse: u8,
    pub sensitivity: f32,
    pub repeat_rate: u32,
    pub repeat_delay: u32,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            kb_layout: "us".to_string(),
            follow_mouse: 1,
            sensitivity: 0.0,
            repeat_rate: 30,
            repeat_delay: 300,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorDirective {
    pub name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub refresh_rate_hz: Option<u32>,
    pub x: i32,
    pub y: i32,
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BezierCurve {
    pub name: String,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl BezierCurve {
    /// Evaluates the cubic-bezier curve at normalized time `t` in `[0.0, 1.0]` using Newton-Raphson solver.
    pub fn sample(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if (t - 0.0).abs() < 1e-6 {
            return 0.0;
        }
        if (t - 1.0).abs() < 1e-6 {
            return 1.0;
        }

        // Solve x(u) = t for parameter u in [0, 1]
        let mut u = t;
        for _ in 0..8 {
            let x = Self::bezier_coord(u, self.x0, self.x1) - t;
            if x.abs() < 1e-5 {
                break;
            }
            let dx = Self::bezier_derivative(u, self.x0, self.x1);
            if dx.abs() < 1e-6 {
                break;
            }
            u = (u - x / dx).clamp(0.0, 1.0);
        }
        Self::bezier_coord(u, self.y0, self.y1)
    }

    #[inline]
    fn bezier_coord(u: f32, p1: f32, p2: f32) -> f32 {
        let inv = 1.0 - u;
        3.0 * inv * inv * u * p1 + 3.0 * inv * u * u * p2 + u * u * u
    }

    #[inline]
    fn bezier_derivative(u: f32, p1: f32, p2: f32) -> f32 {
        let inv = 1.0 - u;
        3.0 * inv * inv * p1 + 6.0 * inv * u * (p2 - p1) + 3.0 * u * u * (1.0 - p2)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimationRule {
    pub name: String, // e.g., "windows", "windowsIn", "windowsOut", "windowsMove", "workspaces", "fade"
    pub enabled: bool,
    pub speed: f32, // deciseconds in Hyprland (e.g. 7 = 700ms, or 3 = 300ms)
    pub curve: String,
    pub style: Option<String>, // e.g., "popin 80%", "slide", "slidevert", "fade"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BindKind {
    Press,   // bind
    Release, // bindr
    Repeat,  // binde
    Mouse,   // bindm
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindRule {
    pub kind: BindKind,
    pub mods: String,
    pub key: String,
    pub dispatcher: String,
    pub arg: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowRuleV2 {
    pub effect: String, // e.g., "float", "opacity 0.9 0.8", "workspace 3", "size 800 600", "center", "nofocus"
    pub matcher: String, // e.g., "class:^(Taskmgr)$", "title:^(Picture-in-Picture)$"
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WallpaperDirective {
    pub monitor: String,
    pub path: String,
    pub transition: String,
}

/// Complete compiled configuration produced from `ConfigDocument`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HyprConfig {
    pub variables: HashMap<String, String>,
    pub general: GeneralConfig,
    pub decoration: DecorationConfig,
    pub animations: AnimationsConfig,
    pub dwindle: DwindleConfig,
    pub master: MasterConfig,
    pub misc: MiscConfig,
    pub input: InputConfig,
    pub monitors: Vec<MonitorDirective>,
    pub exec_once: Vec<String>,
    pub beziers: Vec<BezierCurve>,
    pub animation_rules: Vec<AnimationRule>,
    pub binds: Vec<BindRule>,
    pub window_rules: Vec<WindowRuleV2>,
    pub wallpapers: Vec<WallpaperDirective>,
    #[serde(default)]
    pub theme: crate::theme::ThemeConfig,
    #[serde(default)]
    pub notifications: NotificationsConfig,
    #[serde(default)]
    pub paper: PaperConfig,
}

/// The `paper { }` block: wallpaper layers composited bottom to top by `chickenwm-paper`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaperConfig {
    pub layers: Vec<PaperLayer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaperLayerKind {
    /// A picture file (PNG transparency is kept).
    Image,
    /// The theme's generated wallpaper.
    Generated,
    /// A solid color.
    Color,
    /// The animated shader background (`.wgsl` files are validated; the built-in aurora draws).
    Shader,
}

/// One `layer = kind, source, key = value, …` line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaperLayer {
    pub kind: PaperLayerKind,
    /// Image / shader path, or the color for `color` layers. Empty for `generated`.
    pub source: String,
    /// 0..1
    pub opacity: f32,
    /// `normal` | `multiply` | `screen` | `add` | `overlay`
    pub blend: String,
    /// `cover` | `contain` | `stretch` | `center` | `tile`
    pub fit: String,
    /// Extra size factor for `center` and `tile`.
    pub scale: f32,
    /// Where a smaller-than-screen layer sits: 0 = left/top, 0.5 = middle, 1 = right/bottom.
    pub align_x: f32,
    pub align_y: f32,
    /// Chroma key: pixels close to this color turn transparent.
    pub key: Option<RgbaColor>,
    /// How far (0..1, in chroma) from the key color pixels are fully removed.
    pub tolerance: f32,
    /// Width of the soft edge after `tolerance` (0..1).
    pub softness: f32,
    /// Removes the key color's tint from the edges that remain (green fringes).
    pub despill: bool,
}

impl Default for PaperLayer {
    fn default() -> Self {
        Self {
            kind: PaperLayerKind::Image,
            source: String::new(),
            opacity: 1.0,
            blend: "normal".into(),
            fit: "cover".into(),
            scale: 1.0,
            align_x: 0.5,
            align_y: 0.5,
            key: None,
            tolerance: 0.2,
            softness: 0.1,
            despill: true,
        }
    }
}

/// Splits on commas that aren't inside parentheses, so `rgba(0, 255, 0, 1)` stays whole.
fn split_top_level(v: &str) -> Vec<String> {
    let (mut parts, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for ch in v.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth <= 0 => {
                parts.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts
}

impl PaperLayer {
    /// Parses the value of a `layer =` line: `image, ~/bg/char.png, key = rgb(00ff00), opacity = 0.8`.
    pub fn parse(v: &str) -> Option<Self> {
        let parts = split_top_level(v);
        let mut it = parts.iter();
        let kind = match it.next()?.to_ascii_lowercase().as_str() {
            "image" | "img" => PaperLayerKind::Image,
            "generated" | "theme" => PaperLayerKind::Generated,
            "color" | "colour" | "solid" => PaperLayerKind::Color,
            "shader" | "wgsl" => PaperLayerKind::Shader,
            _ => return None,
        };
        let mut layer = Self { kind, ..Self::default() };
        for part in it {
            let Some((k, val)) = part.split_once('=') else {
                if layer.source.is_empty() {
                    layer.source = part.clone();
                }
                continue;
            };
            let (k, val) = (k.trim().to_ascii_lowercase(), val.trim());
            let num = || val.parse::<f32>().ok();
            match k.as_str() {
                "opacity" | "alpha" => layer.opacity = num().unwrap_or(1.0).clamp(0.0, 1.0),
                "blend" => layer.blend = val.to_ascii_lowercase(),
                "fit" => layer.fit = val.to_ascii_lowercase(),
                "scale" => layer.scale = num().unwrap_or(1.0).clamp(0.01, 50.0),
                "x" | "align_x" => layer.align_x = num().unwrap_or(0.5).clamp(0.0, 1.0),
                "y" | "align_y" => layer.align_y = num().unwrap_or(0.5).clamp(0.0, 1.0),
                "key" | "chroma_key" | "chromakey" => layer.key = RgbaColor::parse(val),
                "tolerance" | "similarity" => layer.tolerance = num().unwrap_or(0.2).clamp(0.0, 1.0),
                "softness" | "smoothness" | "feather" => layer.softness = num().unwrap_or(0.1).clamp(0.0, 1.0),
                "despill" | "spill" => layer.despill = parse_bool(val).unwrap_or(true),
                "source" | "path" | "file" => layer.source = val.to_string(),
                _ => {}
            }
        }
        if matches!(kind, PaperLayerKind::Image | PaperLayerKind::Color) && layer.source.is_empty() {
            return None;
        }
        Some(layer)
    }
}

/// The `notifications { }` block (`chickenwm-notify`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfig {
    pub enabled: bool,
    /// `top-right` | `top-left` | `bottom-right` | `bottom-left` | `top-center` | `bottom-center`
    pub position: String,
    /// Default seconds on screen (0 = until dismissed).
    pub timeout: u32,
    pub max_visible: u32,
    /// Distance from the screen edges (px). Top/bottom margins are measured from the work
    /// area, so notifications sit below roobar.
    pub margin_x: i32,
    pub margin_y: i32,
    /// Show Windows app notifications (toasts) while Explorer isn't running.
    pub windows_toasts: bool,
    /// Apps (display names, case-insensitive) whose notifications are dropped.
    pub blocked: Vec<String>,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            position: "top-right".into(),
            timeout: 6,
            max_visible: 4,
            margin_x: 16,
            margin_y: 12,
            windows_toasts: true,
            blocked: Vec::new(),
        }
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

impl HyprConfig {
    pub fn find_bezier(&self, name: &str) -> BezierCurve {
        self.beziers
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case(name))
            .cloned()
            .unwrap_or(BezierCurve {
                name: "default".to_string(),
                x0: 0.05,
                y0: 0.9,
                x1: 0.1,
                y1: 1.05,
            })
    }

    pub fn apply_category_kv(&mut self, category: &str, key: &str, val: &str) {
        let cat = category.trim().to_ascii_lowercase();
        let k = key.trim().to_ascii_lowercase();
        let v = val.trim();

        match cat.as_str() {
            "general" => match k.as_str() {
                "gaps_in" => {
                    if let Ok(n) = v.parse() {
                        self.general.gaps_in = n;
                    }
                }
                "gaps_out" => {
                    if let Ok(n) = v.parse() {
                        self.general.gaps_out = n;
                    }
                }
                "border_size" => {
                    if let Ok(n) = v.parse() {
                        self.general.border_size = n;
                    }
                }
                "col.active_border" => {
                    self.general.col_active_border = BorderColorSpec::parse(v);
                }
                "col.inactive_border" => {
                    self.general.col_inactive_border = BorderColorSpec::parse(v);
                }
                "layout" => {
                    self.general.layout = v.to_ascii_lowercase();
                }
                "allow_tearing" => {
                    if let Some(b) = parse_bool(v) {
                        self.general.allow_tearing = b;
                    }
                }
                "resize_on_border" => {
                    if let Some(b) = parse_bool(v) {
                        self.general.resize_on_border = b;
                    }
                }
                "no_titlebars" => {
                    if let Some(b) = parse_bool(v) {
                        self.general.no_titlebars = b;
                    }
                }
                _ => {}
            },
            "decoration" => match k.as_str() {
                "rounding" => {
                    if let Ok(n) = v.parse() {
                        self.decoration.rounding = n;
                    }
                }
                "active_opacity" => {
                    if let Ok(n) = v.parse() {
                        self.decoration.active_opacity = n;
                    }
                }
                "inactive_opacity" => {
                    if let Ok(n) = v.parse() {
                        self.decoration.inactive_opacity = n;
                    }
                }
                "fullscreen_opacity" => {
                    if let Ok(n) = v.parse() {
                        self.decoration.fullscreen_opacity = n;
                    }
                }
                "drop_shadow" => {
                    if let Some(b) = parse_bool(v) {
                        self.decoration.drop_shadow = b;
                    }
                }
                "border_anim_speed" => {
                    if let Ok(n) = v.parse() {
                        self.decoration.border_anim_speed = n;
                    }
                }
                "blur_enabled" => {
                    if let Some(b) = parse_bool(v) {
                        self.decoration.blur_enabled = b;
                    }
                }
                "blur_style" => {
                    self.decoration.blur_style = v.to_ascii_lowercase();
                }
                _ => {}
            },
            "decoration:blur" => match k.as_str() {
                "enabled" => {
                    if let Some(b) = parse_bool(v) {
                        self.decoration.blur_enabled = b;
                    }
                }
                "size" => {
                    if let Ok(n) = v.parse() {
                        self.decoration.blur_size = n;
                    }
                }
                "passes" => {
                    if let Ok(n) = v.parse() {
                        self.decoration.blur_passes = n;
                    }
                }
                "style" => {
                    self.decoration.blur_style = v.to_ascii_lowercase();
                }
                _ => {}
            },
            "animations" => match k.as_str() {
                "enabled" => {
                    if let Some(b) = parse_bool(v) {
                        self.animations.enabled = b;
                    }
                }
                "first_launch_animation" => {
                    if let Some(b) = parse_bool(v) {
                        self.animations.first_launch_animation = b;
                    }
                }
                "bezier" => self.apply_directive("bezier", v),
                "animation" => self.apply_directive("animation", v),
                _ => {}
            },
            "dwindle" => match k.as_str() {
                "pseudotile" => {
                    if let Some(b) = parse_bool(v) {
                        self.dwindle.pseudotile = b;
                    }
                }
                "preserve_split" => {
                    if let Some(b) = parse_bool(v) {
                        self.dwindle.preserve_split = b;
                    }
                }
                "force_split" => {
                    if let Ok(n) = v.parse() {
                        self.dwindle.force_split = n;
                    }
                }
                "default_split_ratio" | "split_ratio" => {
                    if let Ok(n) = v.parse() {
                        self.dwindle.default_split_ratio = n;
                    }
                }
                "smart_gaps" => {
                    if let Some(b) = parse_bool(v) {
                        self.dwindle.smart_gaps = b;
                    }
                }
                _ => {}
            },
            "master" => match k.as_str() {
                "mfact" => {
                    if let Ok(n) = v.parse() {
                        self.master.mfact = n;
                    }
                }
                "orientation" => {
                    self.master.orientation = v.to_ascii_lowercase();
                }
                "new_status" => {
                    self.master.new_status = v.to_ascii_lowercase();
                }
                "smart_gaps" => {
                    if let Some(b) = parse_bool(v) {
                        self.master.smart_gaps = b;
                    }
                }
                _ => {}
            },
            "paper" => {
                if k == "layer" {
                    if let Some(layer) = PaperLayer::parse(v) {
                        self.paper.layers.push(layer);
                    }
                }
            }
            "notifications" => match k.as_str() {
                "enabled" => {
                    if let Some(b) = parse_bool(v) {
                        self.notifications.enabled = b;
                    }
                }
                "position" => self.notifications.position = v.to_ascii_lowercase(),
                "timeout" => {
                    if let Ok(n) = v.parse() {
                        self.notifications.timeout = n;
                    }
                }
                "max_visible" => {
                    if let Ok(n) = v.parse::<u32>() {
                        self.notifications.max_visible = n.clamp(1, 12);
                    }
                }
                "margin_x" => {
                    if let Ok(n) = v.parse() {
                        self.notifications.margin_x = n;
                    }
                }
                "margin_y" => {
                    if let Ok(n) = v.parse() {
                        self.notifications.margin_y = n;
                    }
                }
                "windows_toasts" => {
                    if let Some(b) = parse_bool(v) {
                        self.notifications.windows_toasts = b;
                    }
                }
                "blocked" => {
                    self.notifications.blocked = v
                        .split(',')
                        .map(|a| a.trim().to_string())
                        .filter(|a| !a.is_empty())
                        .collect();
                }
                _ => {}
            },
            "theme" => match k.as_str() {
                "accent" | "accent_color" => {
                    if let Some(c) = RgbaColor::parse(v) {
                        self.theme.accent = RgbaColor { a: 255, ..c };
                    }
                }
                "mode" => {
                    if let Some(m) = crate::theme::ThemeMode::parse(v) {
                        self.theme.mode = m;
                    }
                }
                "icon_theme" | "icons" => self.theme.icon_theme = v.to_string(),
                "preset" | "scheme" | "colorscheme" => {
                    let v = v.trim();
                    self.theme.preset = if v.is_empty() || v.eq_ignore_ascii_case("none") || v.eq_ignore_ascii_case("chickenwm") {
                        String::new()
                    } else {
                        crate::theme_presets::find(v).map_or_else(|| v.to_ascii_lowercase(), |p| p.id.to_string())
                    };
                }
                "app_modes" => self.theme.app_modes = crate::theme::ThemeConfig::parse_app_modes(v),
                _ => {}
            },
            "misc" => match k.as_str() {
                "vfr" => {
                    if let Some(b) = parse_bool(v) {
                        self.misc.vfr = b;
                    }
                }
                "fps_limit" => {
                    if let Ok(n) = v.parse::<u32>() {
                        self.misc.fps_limit = n;
                    } else if v.eq_ignore_ascii_case("unlimited") {
                        self.misc.fps_limit = 500;
                    }
                }
                "suppress_explorer" => {
                    if let Some(b) = parse_bool(v) {
                        self.misc.suppress_explorer = b;
                    }
                }
                "replace_winlogon_shell" => {
                    if let Some(b) = parse_bool(v) {
                        self.misc.replace_winlogon_shell = b;
                    }
                }
                "register_files_handler" => {
                    if let Some(b) = parse_bool(v) {
                        self.misc.register_files_handler = b;
                    }
                }
                "run_as_admin" => {
                    if let Some(b) = parse_bool(v) {
                        self.misc.run_as_admin = b;
                    }
                }
                "dwm_hook_enabled" | "compositor_hook" => {
                    if let Some(b) = parse_bool(v) {
                        self.misc.dwm_hook_enabled = b;
                    }
                }
                "resize_preview_classes" => {
                    self.misc.resize_preview_classes = v
                        .split(',')
                        .map(|c| c.trim().to_string())
                        .filter(|c| !c.is_empty())
                        .collect();
                }
                _ => {}
            },
            "input" => match k.as_str() {
                "kb_layout" => self.input.kb_layout = v.to_string(),
                "follow_mouse" => {
                    if let Ok(n) = v.parse() {
                        self.input.follow_mouse = n;
                    }
                }
                "sensitivity" => {
                    if let Ok(n) = v.parse() {
                        self.input.sensitivity = n;
                    }
                }
                "repeat_rate" => {
                    if let Ok(n) = v.parse() {
                        self.input.repeat_rate = n;
                    }
                }
                "repeat_delay" => {
                    if let Ok(n) = v.parse() {
                        self.input.repeat_delay = n;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn apply_directive(&mut self, keyword: &str, val: &str) {
        let kw = keyword.trim().to_ascii_lowercase();
        let v = val.trim();

        match kw.as_str() {
            "exec-once" | "exec" => {
                if !v.is_empty() {
                    self.exec_once.push(v.to_string());
                }
            }
            "monitor" => {
                // Format: `name, 1920x1080@144, 0x0, 1` or `, preferred, auto, 1`
                let parts: Vec<&str> = v.split(',').map(|s| s.trim()).collect();
                let name = parts.first().copied().unwrap_or("").to_string();
                let mut width = None;
                let mut height = None;
                let mut refresh_rate_hz = None;
                if let Some(res) = parts.get(1) {
                    let (dim_part, hz_part) = match res.split_once('@') {
                        Some((d, h)) => (d, Some(h)),
                        None => (*res, None),
                    };
                    if let Some((w_str, h_str)) = dim_part.split_once('x') {
                        width = w_str.parse().ok();
                        height = h_str.parse().ok();
                    }
                    if let Some(hz_s) = hz_part {
                        refresh_rate_hz = hz_s.parse::<f32>().ok().map(|f| f.round() as u32);
                    }
                }
                let mut x = 0;
                let mut y = 0;
                if let Some(pos) = parts.get(2) {
                    if let Some((xs, ys)) = pos.split_once('x') {
                        x = xs.parse().unwrap_or(0);
                        y = ys.parse().unwrap_or(0);
                    }
                }
                let scale = parts
                    .get(3)
                    .and_then(|s| s.parse::<f32>().ok())
                    .unwrap_or(1.0);
                self.monitors.push(MonitorDirective {
                    name,
                    width,
                    height,
                    refresh_rate_hz,
                    x,
                    y,
                    scale,
                });
            }
            "bezier" => {
                // Format: `myCurve, 0.05, 0.9, 0.1, 1.05`
                let parts: Vec<&str> = v.split(',').map(|s| s.trim()).collect();
                if parts.len() >= 5 {
                    if let (Ok(x0), Ok(y0), Ok(x1), Ok(y1)) = (
                        parts[1].parse(),
                        parts[2].parse(),
                        parts[3].parse(),
                        parts[4].parse(),
                    ) {
                        self.beziers.push(BezierCurve {
                            name: parts[0].to_string(),
                            x0,
                            y0,
                            x1,
                            y1,
                        });
                    }
                }
            }
            "animation" => {
                // Format: `windows, 1, 7, myCurve, popin 80%`
                let parts: Vec<&str> = v.split(',').map(|s| s.trim()).collect();
                if parts.len() >= 4 {
                    let name = parts[0].to_string();
                    let enabled = parse_bool(parts[1]).unwrap_or(true);
                    let speed = parts[2].parse::<f32>().unwrap_or(5.0);
                    let curve = parts[3].to_string();
                    let style = parts
                        .get(4)
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty());
                    self.animation_rules.push(AnimationRule {
                        name,
                        enabled,
                        speed,
                        curve,
                        style,
                    });
                }
            }
            "bind" | "bindr" | "binde" | "bindm" | "bindl" => {
                let kind = match kw.as_str() {
                    "bindr" => BindKind::Release,
                    "binde" => BindKind::Repeat,
                    "bindm" => BindKind::Mouse,
                    _ => BindKind::Press,
                };
                // Format: `$mainMod, Q, exec, wt.exe` or `$mainMod, mouse:272, movewindow`
                let mut parts = v.splitn(4, ',').map(|s| s.trim());
                let mods = parts.next().unwrap_or("").to_string();
                let key = parts.next().unwrap_or("").to_string();
                let dispatcher = parts.next().unwrap_or("").to_string();
                let arg = parts.next().unwrap_or("").to_string();
                if !key.is_empty() && !dispatcher.is_empty() {
                    self.binds.push(BindRule {
                        kind,
                        mods,
                        key,
                        dispatcher,
                        arg,
                    });
                }
            }
            "windowrulev2" | "windowrule" => {
                if let Some((effect, matcher)) = v.split_once(',') {
                    self.window_rules.push(WindowRuleV2 {
                        effect: effect.trim().to_string(),
                        matcher: matcher.trim().to_string(),
                    });
                }
            }
            "wallpaper" => {
                let parts: Vec<&str> = v.split(',').map(|s| s.trim()).collect();
                if parts.len() >= 2 {
                    self.wallpapers.push(WallpaperDirective {
                        monitor: parts[0].to_string(),
                        path: parts[1].to_string(),
                        transition: parts.get(2).copied().unwrap_or("fade").to_string(),
                    });
                }
            }
            _ => {}
        }
    }
}

/// Phase 10.3: Default `~/.config/chickenwm/chickenwm.conf` content generated on first boot.
pub const DEFAULT_CHICKENWM_CONF: &str = r#"# =============================================================================
# ChickenWM — Master Configuration (~/.config/chickenwm/chickenwm.conf)
# =============================================================================
# Dynamic variables
$mainMod = SUPER
$term = wt.exe
$files = chickenwm-files.exe
$launcher = chickenwm-launcher.exe
$settings = chickenwm-settings.exe
$bar = roobar.exe

# Monitor configuration (auto-detects native refresh rate by default)
monitor = , preferred, auto, 1

# Startup applications
exec-once = chickenwm-paper.exe
exec-once = roobar.exe
exec-once = chickenwm-launcher.exe --daemon

general {
    gaps_in = 5
    gaps_out = 10
    border_size = 2
    col.active_border = rgba(242321ff)
    col.inactive_border = rgba(24232133)
    layout = dwindle
    allow_tearing = false
    resize_on_border = true
    no_titlebars = true
}

# System theme: one accent color themes every module (bar, launcher, files, browser, settings,
# window borders, icons). Apply changes from Settings → Appearance, or edit and re-apply.
theme {
    accent = rgb(242321)
    mode = light
    icon_theme = chickenwm
}

# Notifications (chickenwm-notify). Styling: ~/.config/chickenwm/notifications.css
notifications {
    position = top-right
    timeout = 6
    max_visible = 4
}

decoration {
    rounding = 10
    active_opacity = 1.0
    inactive_opacity = 0.92
    fullscreen_opacity = 1.0
    drop_shadow = true
    border_anim_speed = 1.0

    blur {
        enabled = true
        size = 8
        passes = 2
        style = acrylic
    }
}

animations {
    enabled = true
    first_launch_animation = true

    bezier = myBezier, 0.05, 0.9, 0.1, 1.05
    bezier = overshot, 0.13, 0.99, 0.29, 1.1
    bezier = smoothOut, 0.36, 0, 0.66, -0.56

    animation = windows, 1, 5, myBezier, popin 80%
    animation = windowsIn, 1, 5, overshot, popin 80%
    animation = windowsOut, 1, 4, smoothOut, popin 80%
    animation = windowsMove, 1, 4, myBezier, slide
    animation = workspaces, 1, 5, myBezier, slide
    animation = fade, 1, 4, myBezier
}

dwindle {
    pseudotile = true
    preserve_split = true
    force_split = 0
    default_split_ratio = 1.0
    smart_gaps = false
}

master {
    mfact = 0.55
    orientation = left
    new_status = slave
    smart_gaps = false
}

misc {
    # Variable Frame Rate: automatically sleep the frame scheduler when animations settle
    vfr = true
    # 0 = dynamically sync to active monitor's native refresh rate (e.g. 60Hz, 144Hz, 240Hz)
    fps_limit = 0
    # Kill explorer.exe while chickenwm-rooster runs (restored on Ctrl+Alt+Shift+Esc)
    suppress_explorer = true
    replace_winlogon_shell = false
    register_files_handler = false
    # Start elevated (administrator) so admin windows can be tiled; toggle it in Settings
    run_as_admin = false
    # Anti-Cheat Safe Switch: false = Standard Win32/DirectComposition mode, true = Experimental uDWM PDB Hook
    dwm_hook_enabled = false
    # Window classes resized with a stretched GPU preview while dragging (real resize on release)
    resize_preview_classes = MozillaWindowClass
}

input {
    kb_layout = us
    follow_mouse = 1
    sensitivity = 0.0
    repeat_rate = 30
    repeat_delay = 300
}

# Window rules (Hyprland windowrulev2 syntax)
windowrulev2 = float, title:^(Picture-in-Picture)$
windowrulev2 = float, class:^(chickenwm-settings)$
windowrulev2 = float, class:^(roobar)$

# Keybindings
bind = $mainMod, Return, exec, $term
bind = $mainMod, Q, killactive,
bind = $mainMod, E, exec, $files
bind = $mainMod, D, launcher,
bind = ALT, Space, launcher,
bind = $mainMod, I, settings,
bind = $mainMod, N, notifications,
bind = $mainMod, V, togglefloating,
bind = $mainMod, F, fullscreen, 0
bind = $mainMod, P, pseudo,
bind = $mainMod, J, togglesplit,
bind = $mainMod SHIFT, S, exec, chickenwm-shot.exe
bind = , Print, exec, chickenwm-shot.exe
bind = SHIFT, Print, exec, chickenwm-shot.exe --full

# Focus movement
bind = $mainMod, Left, movefocus, l
bind = $mainMod, Right, movefocus, r
bind = $mainMod, Up, movefocus, u
bind = $mainMod, Down, movefocus, d

# Window movement
bind = $mainMod SHIFT, Left, movewindow, l
bind = $mainMod SHIFT, Right, movewindow, r
bind = $mainMod SHIFT, Up, movewindow, u
bind = $mainMod SHIFT, Down, movewindow, d

# Workspaces 1..10
bind = $mainMod, 1, workspace, 1
bind = $mainMod, 2, workspace, 2
bind = $mainMod, 3, workspace, 3
bind = $mainMod, 4, workspace, 4
bind = $mainMod, 5, workspace, 5
bind = $mainMod, 6, workspace, 6
bind = $mainMod, 7, workspace, 7
bind = $mainMod, 8, workspace, 8
bind = $mainMod, 9, workspace, 9
bind = $mainMod, 0, workspace, 10

# Move active window to workspace 1..10
bind = $mainMod SHIFT, 1, movetoworkspace, 1
bind = $mainMod SHIFT, 2, movetoworkspace, 2
bind = $mainMod SHIFT, 3, movetoworkspace, 3
bind = $mainMod SHIFT, 4, movetoworkspace, 4
bind = $mainMod SHIFT, 5, movetoworkspace, 5
bind = $mainMod SHIFT, 6, movetoworkspace, 6
bind = $mainMod SHIFT, 7, movetoworkspace, 7
bind = $mainMod SHIFT, 8, movetoworkspace, 8
bind = $mainMod SHIFT, 9, movetoworkspace, 9
bind = $mainMod SHIFT, 0, movetoworkspace, 10

# Resize active window with keyboard
binde = $mainMod CTRL, Left, resizeactive, -40 0
binde = $mainMod CTRL, Right, resizeactive, 40 0
binde = $mainMod CTRL, Up, resizeactive, 0 -40
binde = $mainMod CTRL, Down, resizeactive, 0 40

# Mouse bindings ($mainMod + LMB to move, $mainMod + RMB to resize)
bindm = $mainMod, mouse:272, movewindow
bindm = $mainMod, mouse:273, resizewindow
"#;

/// Ensures `~/.config/chickenwm/chickenwm.conf` exists (creating default if missing),
/// then parses and returns both the lossless `ConfigDocument` AST and compiled `HyprConfig`.
pub fn load_or_init_config() -> Result<(ConfigDocument, HyprConfig)> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create config directory {}", dir.display()))?;
    let path = config_file_path();
    if !path.exists() {
        std::fs::write(&path, DEFAULT_CHICKENWM_CONF)
            .with_context(|| format!("Failed to write default {}", path.display()))?;
    }
    load_config_from_path(&path)
}

pub fn load_config_from_path(path: &Path) -> Result<(ConfigDocument, HyprConfig)> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file {}", path.display()))?;
    let doc = ConfigDocument::parse(&raw)?;
    let compiled = doc.compile();
    Ok((doc, compiled))
}

pub fn save_config_document(doc: &ConfigDocument) -> Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = config_file_path();
    let content = doc.to_lossless_string();
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_layers_parse() {
        let cfg = ConfigDocument::parse(
            "paper {\n    layer = image, ~/bg/sky.png\n    layer = image, ~/bg/char.png, key = rgb(00ff00), tolerance = 0.25, softness = 0.1\n    layer = shader, ~/shaders/rain.wgsl, opacity = 0.4, blend = screen\n    layer = color, rgba(0, 0, 0, 0.3)\n    layer = bogus, x\n}\n",
        )
        .unwrap()
        .compile();
        let l = &cfg.paper.layers;
        assert_eq!(l.len(), 4);
        assert_eq!(l[1].key, Some(RgbaColor { r: 0, g: 255, b: 0, a: 255 }));
        assert_eq!(l[1].tolerance, 0.25);
        assert_eq!(l[2].kind, PaperLayerKind::Shader);
        assert_eq!((l[2].opacity, l[2].blend.as_str()), (0.4, "screen"));
        assert_eq!(l[3].source, "rgba(0, 0, 0, 0.3)");
    }

    #[test]
    fn test_parse_and_lossless_roundtrip() {
        let doc = ConfigDocument::parse(DEFAULT_CHICKENWM_CONF).expect("should parse default config");
        let serialized = doc.to_lossless_string();
        assert_eq!(serialized.trim(), DEFAULT_CHICKENWM_CONF.trim());

        let cfg = doc.compile();
        assert_eq!(cfg.general.gaps_in, 5);
        assert_eq!(cfg.general.gaps_out, 10);
        assert_eq!(cfg.general.border_size, 2);
        assert_eq!(cfg.general.layout, "dwindle");
        assert_eq!(cfg.decoration.rounding, 10);
        assert_eq!(cfg.decoration.blur_enabled, true);
        assert_eq!(cfg.misc.vfr, true);
        assert_eq!(cfg.misc.fps_limit, 0);
        assert!(!cfg.binds.is_empty());
        assert_eq!(cfg.binds[0].mods, "SUPER");
        assert_eq!(cfg.binds[0].arg, "wt.exe");
    }

    #[test]
    fn test_two_way_ast_mutation_preserves_comments() {
        let mut doc = ConfigDocument::parse(DEFAULT_CHICKENWM_CONF).unwrap();
        doc.set_category_key("misc", "fps_limit", "144");
        doc.set_category_key("general", "gaps_in", "12");
        let text = doc.to_lossless_string();
        assert!(text.contains("# Variable Frame Rate:"));
        let cfg = doc.compile();
        assert_eq!(cfg.misc.fps_limit, 144);
        assert_eq!(cfg.general.gaps_in, 12);
    }

    #[test]
    fn test_cubic_bezier_evaluator() {
        let curve = BezierCurve {
            name: "myBezier".to_string(),
            x0: 0.05,
            y0: 0.9,
            x1: 0.1,
            y1: 1.05,
        };
        assert!((curve.sample(0.0) - 0.0).abs() < 1e-4);
        assert!((curve.sample(1.0) - 1.0).abs() < 1e-4);
        let mid = curve.sample(0.5);
        assert!(mid > 0.7 && mid < 1.15);
    }
}
