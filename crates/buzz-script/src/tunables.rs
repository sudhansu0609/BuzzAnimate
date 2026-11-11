//! A command's tunables: the numbers at the top of a JSFL script (JSFL_COMMANDS_PLAN J5).
//!
//! A shelf of Animate commands keeps its knobs as plain declarations --
//! `var moveAmount = 5; // pixels` -- or as members of a settings object
//! (`CONFIG = { moveFrames: 6, ... }`). Changing how far an eye moves meant
//! opening the script in an editor. This reads those declarations so a window
//! can offer them, and rewrites the literal in a **copy** of the script text
//! when a value is overridden. The user's script file is never touched.
//!
//! Only declarations whose right-hand side is a literal are tunables: a number,
//! a quoted string, `true`/`false`, or a flat list of numbers. Anything computed
//! is code, not a setting.

/// What kind of value a tunable holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Number,
    Bool,
    Text,
    /// `[1, 2, 3]` -- kept as the text between the brackets.
    Numbers,
}

/// One setting found in a script.
#[derive(Debug, Clone, PartialEq)]
pub struct Tunable {
    /// `moveFrames`, or `CONFIG.moveAmount` for a member of a settings object.
    pub name: String,
    pub kind: Kind,
    /// The literal as written in the script (`6`, `"left"`, `true`, `[1, 2]`).
    pub default: String,
    /// The comment on that line, else the comment line above it.
    pub label: String,
    /// 1-based line in the source.
    pub line: usize,
    /// Byte range of the literal in the source.
    span: (usize, usize),
}

impl Tunable {
    /// The default as a number, for sizing a slider.
    pub fn default_number(&self) -> Option<f64> {
        self.default.trim().parse().ok()
    }
}

/// A literal at the start of `text`: its kind and byte length.
fn literal_at(text: &str) -> Option<(Kind, usize)> {
    let t = text;
    let bytes = t.as_bytes();
    let first = *bytes.first()?;
    if first == b'"' || first == b'\'' {
        let close = t[1..].find(first as char)?;
        if t[1..1 + close].contains('\\') {
            return None;
        }
        return Some((Kind::Text, close + 2));
    }
    if first == b'[' {
        let close = t.find(']')?;
        let inner = &t[1..close];
        let ok = !inner.trim().is_empty()
            && inner.split(',').all(|p| p.trim().parse::<f64>().is_ok());
        return ok.then_some((Kind::Numbers, close + 1));
    }
    for word in ["true", "false"] {
        if t.starts_with(word)
            && !t[word.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
        {
            return Some((Kind::Bool, word.len()));
        }
    }
    // A number: -?digits(.digits)?
    let mut i = 0;
    if bytes.get(i) == Some(&b'-') {
        i += 1;
    }
    let digits_start = i;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    // `5 * x`, `5px` or `5.toFixed()` are code, not a literal.
    let rest = t[i..].trim_start();
    if !(rest.is_empty()
        || rest.starts_with(';')
        || rest.starts_with(',')
        || rest.starts_with("//")
        || rest.starts_with('}'))
    {
        return None;
    }
    Some((Kind::Number, i))
}

/// Does `literal` read as a value of `kind`?
pub fn literal_is(kind: Kind, literal: &str) -> bool {
    matches!(literal_at(literal.trim()), Some((k, n)) if k == kind && n == literal.trim().len())
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// Names that are loop counters and scratch values, not settings.
fn scratch(name: &str) -> bool {
    let leaf = name.rsplit('.').next().unwrap_or(name);
    leaf.len() <= 2
        || matches!(
            leaf,
            "idx" | "index" | "count" | "total" | "found" | "changed" | "result" | "counter"
        )
}

/// After the literal: `;`/`,` then an optional `// comment`.
fn trailing_comment(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(';').or_else(|| rest.strip_prefix(',')).unwrap_or(rest);
    let rest = rest.trim_start();
    if rest.is_empty() {
        return Some(String::new());
    }
    let c = rest.strip_prefix("//")?;
    Some(c.trim_start_matches('/').trim().to_string())
}

/// Read every tunable out of `source`.
pub fn parse(source: &str) -> Vec<Tunable> {
    let mut out: Vec<Tunable> = Vec::new();
    let mut block: Option<String> = None;
    let mut previous_comment = String::new();
    let mut in_block_comment = false;
    let mut offset = 0usize;

    for (n, raw_line) in source.split_inclusive('\n').enumerate() {
        let line_start = offset;
        offset += raw_line.len();
        let line = raw_line.trim_end_matches(['\n', '\r']);
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();

        if in_block_comment {
            if trimmed.contains("*/") {
                in_block_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_block_comment = true;
            }
            continue;
        }
        if let Some(c) = trimmed.strip_prefix("//") {
            previous_comment = c.trim_start_matches('/').trim().to_string();
            continue;
        }
        let label_above = std::mem::take(&mut previous_comment);

        // Members of a settings object.
        if let Some(obj) = &block {
            if trimmed.starts_with('}') {
                block = None;
                continue;
            }
            if let Some((key, rest)) = trimmed.split_once(':') {
                let key = key.trim();
                let rest_trim = rest.trim_start();
                if is_ident(key)
                    && let Some((kind, len)) = literal_at(rest_trim)
                    && let Some(comment) = trailing_comment(&rest_trim[len..])
                {
                    let lit_start = line_start
                        + indent
                        + key.len()
                        + 1
                        + (rest.len() - rest_trim.len());
                    out.push(Tunable {
                        name: format!("{obj}.{key}"),
                        kind,
                        default: rest_trim[..len].to_string(),
                        label: if comment.is_empty() { label_above } else { comment },
                        line: n + 1,
                        span: (lit_start, lit_start + len),
                    });
                }
            }
            continue;
        }

        // `var NAME = ...`
        let Some(decl) = ["var ", "let ", "const "]
            .iter()
            .find_map(|k| trimmed.strip_prefix(k))
        else {
            continue;
        };
        if trimmed.contains("for (") || trimmed.contains("for(") {
            continue;
        }
        let Some((name, rest)) = decl.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !is_ident(name) {
            continue;
        }
        let rest_trim = rest.trim_start();
        if rest_trim.trim_end() == "{" {
            block = Some(name.to_string());
            continue;
        }
        let Some((kind, len)) = literal_at(rest_trim) else {
            continue;
        };
        let Some(comment) = trailing_comment(&rest_trim[len..]) else {
            continue;
        };
        if scratch(name) {
            continue;
        }
        // `trimmed` begins with the keyword, so the literal sits where
        // `rest_trim` begins within it.
        let lit_start = line_start + indent + (trimmed.len() - rest_trim.len());
        out.push(Tunable {
            name: name.to_string(),
            kind,
            default: rest_trim[..len].to_string(),
            label: if comment.is_empty() { label_above } else { comment },
            line: n + 1,
            span: (lit_start, lit_start + len),
        });
    }

    // A name declared twice is code that reuses a variable, not a setting.
    let names: Vec<String> = out.iter().map(|t| t.name.clone()).collect();
    out.retain(|t| names.iter().filter(|n| **n == t.name).count() == 1);
    out
}

/// `source` with the literal of each overridden tunable replaced. An override
/// naming something the script does not declare, or whose value is not the
/// right kind of literal, is ignored -- the script keeps its own value.
pub fn apply(source: &str, overrides: &[(String, String)]) -> String {
    if overrides.is_empty() {
        return source.to_string();
    }
    let found = parse(source);
    let mut edits: Vec<((usize, usize), &str)> = Vec::new();
    for (name, literal) in overrides {
        if let Some(t) = found.iter().find(|t| &t.name == name)
            && literal_is(t.kind, literal)
        {
            edits.push((t.span, literal.trim()));
        }
    }
    edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.0));
    let mut text = source.to_string();
    for ((a, b), lit) in edits {
        text.replace_range(a..b, lit);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_declarations_and_object_members_with_their_comments() {
        let src = "var moveFrames = 6;\n\
                   // how far, in pixels\n\
                   var moveAmount = 5;\n\
                   var CONFIG = {\n\
                   \x20   moveFrames: moveFrames,\n\
                   \x20   browFrames: 10, // frames per brow move\n\
                   \x20   name: \"left\",\n\
                   };\n\
                   for (var i = 0; i < 3; i++) {}\n\
                   var computed = moveFrames * 2;\n";
        let t = parse(src);
        let names: Vec<_> = t.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["moveFrames", "moveAmount", "CONFIG.browFrames", "CONFIG.name"]);
        assert_eq!(t[1].label, "how far, in pixels");
        assert_eq!(t[2].label, "frames per brow move");
        assert_eq!(t[3].kind, Kind::Text);
    }

    #[test]
    fn apply_rewrites_only_the_literal() {
        let src = "var moveA = 5; // px\nvar CONFIG = {\n  bb: 10,\n};\n";
        let out = apply(
            src,
            &[("moveA".into(), "9".into()), ("CONFIG.bb".into(), "12".into()), ("a2".into(), "1".into())],
        );
        assert_eq!(out, "var moveA = 9; // px\nvar CONFIG = {\n  bb: 12,\n};\n");
        // The wrong kind of value is ignored.
        assert_eq!(apply(src, &[("moveA".into(), "\"x\"".into())]), src);
    }
}
