//! Language adapter abstraction for structured trace analysis.
//!
//! Morph's structured trace layer needs to reason about code artifacts
//! (functions, classes, files) without hard-coding assumptions about any
//! specific language. Each supported language implements [`LanguageAdapter`]
//! with best-effort symbol extraction and slicing heuristics.
//!
//! Why this matters for replay/eval systems (like tap):
//!
//! * **Why `target_symbol` matters** — for localized coding tasks
//!   (fix a bug in one function, rename a method), the replay prompt and
//!   eval artifacts need the function source rather than a whole-file diff.
//!   An adapter lets us slice that function out of a full file body.
//! * **Why the abstraction is language-general** — while we implement
//!   Python first, the MCP tool contracts, metadata schema, and CLI
//!   outputs are language-agnostic. Adding JS/TS, Go, Java, Rust, etc. is
//!   a matter of implementing this trait.
//!
//! The adapter returns best-effort results; callers MUST treat them as
//! heuristic hints, not guarantees of correctness.

use std::collections::BTreeSet;

/// A symbol extracted from source code. `kind` is implementation-defined
/// (e.g. "function", "method", "class", "async_function").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    /// 1-indexed starting line of the symbol's definition.
    pub start_line: usize,
    /// 1-indexed ending line (inclusive).
    pub end_line: usize,
}

/// Language adapter trait. Each impl owns the language-specific heuristics
/// (tokenization, indentation, brace matching, etc.) needed to extract and
/// slice symbols. The core structured-trace layer treats it as a black box.
pub trait LanguageAdapter: Send + Sync {
    /// Human-readable language name (e.g. "python", "javascript").
    fn name(&self) -> &'static str;

    /// Whether this adapter recognizes a filename (by extension).
    fn detect_language(&self, filename: &str) -> bool;

    /// Return all top-level symbols (functions/classes/methods) detected in
    /// `source`. Best-effort. Empty vec when nothing matches.
    fn extract_symbols(&self, source: &str) -> Vec<Symbol>;

    /// Pick the most likely "target" symbol given optional prompt text / hint.
    /// Used by heuristics that classify a trace as a localized
    /// `single_function` task. Returns `None` if no symbol stands out.
    fn detect_target_symbol(&self, source: &str, hint: Option<&str>) -> Option<String>;

    /// Return the source text for a symbol (function/class body). Returns
    /// `None` when the symbol is not found. The slice SHOULD include the
    /// definition line and the full body.
    fn slice_symbol(&self, source: &str, symbol: &str) -> Option<String>;

    /// Extract likely symbol names referenced in a free-form text (user
    /// prompt, comment, LLM response). Useful for guessing the target
    /// symbol when the source is not available.
    fn extract_symbol_references(&self, text: &str) -> Vec<String>;
}

// -------- Python adapter --------

/// Python language adapter.
///
/// Heuristics:
/// * Functions: lines matching `^\s*(async\s+)?def\s+<name>\s*\(`.
/// * Classes:  lines matching `^\s*class\s+<name>\s*[:\(]`.
/// * Body extent: consecutive lines indented deeper than the definition
///   line, plus blank lines between them, until a less/equally indented
///   non-blank line appears.
///
/// These rules cover the common cases (top-level defs, nested methods,
/// decorators above the def) well enough for replay-eval metadata. They
/// are NOT a full Python parser.
pub struct PythonLanguageAdapter;

impl PythonLanguageAdapter {
    pub fn new() -> Self {
        PythonLanguageAdapter
    }
}

impl Default for PythonLanguageAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn leading_indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// Parse `def <name>(` / `async def <name>(` / `class <name>(:` at the
/// start of a line (after whitespace). Returns `(kind, name)`.
fn parse_py_def(line: &str) -> Option<(&'static str, String)> {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("async def ") {
        extract_ident(rest).map(|n| ("async_function", n))
    } else if let Some(rest) = trimmed.strip_prefix("def ") {
        extract_ident(rest).map(|n| ("function", n))
    } else if let Some(rest) = trimmed.strip_prefix("class ") {
        extract_ident(rest).map(|n| ("class", n))
    } else {
        None
    }
}

fn extract_ident(s: &str) -> Option<String> {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    if end == 0 {
        None
    } else {
        Some(s[..end].to_string())
    }
}

impl LanguageAdapter for PythonLanguageAdapter {
    fn name(&self) -> &'static str {
        "python"
    }

    fn detect_language(&self, filename: &str) -> bool {
        let lower = filename.to_ascii_lowercase();
        lower.ends_with(".py") || lower.ends_with(".pyi")
    }

    fn extract_symbols(&self, source: &str) -> Vec<Symbol> {
        let lines: Vec<&str> = source.lines().collect();
        let mut out = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            if let Some((kind, name)) = parse_py_def(line) {
                let def_indent = leading_indent(line);
                let start = i + 1;

                // Walk forward while lines are indented deeper than the def,
                // or blank. Stop at the first non-blank line at <= def_indent.
                let mut end = start;
                for (j, body_line) in lines.iter().enumerate().skip(i + 1) {
                    if body_line.trim().is_empty() {
                        continue;
                    }
                    let ind = leading_indent(body_line);
                    if ind > def_indent {
                        end = j + 1;
                    } else {
                        break;
                    }
                }
                if end == start {
                    end = start;
                }
                out.push(Symbol {
                    name,
                    kind: kind.to_string(),
                    start_line: start,
                    end_line: end,
                });
            }
        }
        out
    }

    fn detect_target_symbol(&self, source: &str, hint: Option<&str>) -> Option<String> {
        let symbols = self.extract_symbols(source);
        if symbols.is_empty() {
            return None;
        }

        if let Some(hint_text) = hint {
            // If any symbol name appears in the hint, prefer the longest match
            // (avoids picking "get" when "get_user_by_id" is what's mentioned).
            let mut best: Option<&Symbol> = None;
            for s in &symbols {
                if hint_text.contains(&s.name) {
                    match best {
                        None => best = Some(s),
                        Some(b) if s.name.len() > b.name.len() => best = Some(s),
                        _ => {}
                    }
                }
            }
            if let Some(s) = best {
                return Some(s.name.clone());
            }
        }

        // With no hint match, only claim a target when there's exactly one
        // function/method (a strong "single function file" signal).
        let funcs: Vec<&Symbol> = symbols
            .iter()
            .filter(|s| s.kind == "function" || s.kind == "async_function")
            .collect();
        if funcs.len() == 1 {
            return Some(funcs[0].name.clone());
        }
        None
    }

    fn slice_symbol(&self, source: &str, symbol: &str) -> Option<String> {
        let lines: Vec<&str> = source.lines().collect();
        let symbols = self.extract_symbols(source);
        let sym = symbols.iter().find(|s| s.name == symbol)?;
        let start_idx = sym.start_line.saturating_sub(1);
        let end_idx = sym.end_line.min(lines.len());
        // Include decorator lines immediately above the def (pythonic)
        let mut first = start_idx;
        while first > 0 {
            let prev = lines[first - 1].trim_start();
            if prev.starts_with('@') {
                first -= 1;
            } else {
                break;
            }
        }
        Some(lines[first..end_idx].join("\n"))
    }

    fn extract_symbol_references(&self, text: &str) -> Vec<String> {
        // Find identifier-like tokens; filter to those that look like
        // function/method references. Heuristic: the token is mentioned in
        // a way suggesting a symbol ("`foo`", "foo(", "def foo", etc.).
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut out = Vec::new();

        // Backtick-quoted identifiers
        for chunk in text.split('`').skip(1).step_by(2) {
            if let Some(id) = extract_ident(chunk) {
                if !seen.contains(&id) && is_plausible_symbol(&id) {
                    seen.insert(id.clone());
                    out.push(id);
                }
            }
        }

        // foo(...) style references
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c.is_ascii_alphabetic() || c == '_' {
                let start = i;
                while i < bytes.len() {
                    let ch = bytes[i] as char;
                    if ch.is_ascii_alphanumeric() || ch == '_' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                if i < bytes.len() && bytes[i] as char == '(' {
                    let id = &text[start..i];
                    if !seen.contains(id) && is_plausible_symbol(id) {
                        seen.insert(id.to_string());
                        out.push(id.to_string());
                    }
                }
            } else {
                i += 1;
            }
        }
        out
    }
}

/// Filter obvious stopwords out of "symbol-like" identifiers we pulled
/// from free-form text. Keeps snake_case / camelCase / PascalCase tokens.
fn is_plausible_symbol(s: &str) -> bool {
    if s.is_empty() || s.len() > 80 {
        return false;
    }
    // Common English words we don't want to treat as symbols.
    const STOP: &[&str] = &[
        "the", "this", "that", "and", "for", "but", "use", "used", "with",
        "from", "into", "onto", "over", "when", "then", "than", "not",
        "can", "will", "would", "should", "it", "is", "be", "do", "if",
        "in", "on", "at", "of", "to", "by", "as", "a", "an", "or",
        "def", "class", "return", "True", "False", "None", "print",
        "pass", "try", "except", "raise", "import", "from",
    ];
    if STOP.contains(&s) || STOP.contains(&s.to_ascii_lowercase().as_str()) {
        return false;
    }
    // Reject purely numeric or single-char names.
    if s.len() < 2 {
        return false;
    }
    s.chars().next().map(|c| c.is_ascii_alphabetic() || c == '_').unwrap_or(false)
}

/// Pick a language adapter for a filename. Returns `None` if no adapter
/// matches. The set of built-in adapters is intentionally narrow; more
/// will be added alongside each new language implementation.
pub fn adapter_for_filename(filename: &str) -> Option<Box<dyn LanguageAdapter>> {
    let candidates: Vec<Box<dyn LanguageAdapter>> = vec![Box::new(PythonLanguageAdapter::new())];
    candidates.into_iter().find(|a| a.detect_language(filename))
}

/// All built-in adapters (for iteration / diagnostics).
pub fn builtin_adapters() -> Vec<Box<dyn LanguageAdapter>> {
    vec![Box::new(PythonLanguageAdapter::new())]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
import os


def list_tasks(db):
    rows = db.fetch('tasks')
    return [r.title for r in rows]


class TaskRepo:
    def __init__(self):
        self.db = None

    def add(self, title):
        self.db.insert(title)


async def refresh():
    await fetch_once()
";

    #[test]
    fn python_extract_symbols() {
        let a = PythonLanguageAdapter::new();
        let syms = a.extract_symbols(SRC);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"list_tasks"));
        assert!(names.contains(&"TaskRepo"));
        assert!(names.contains(&"refresh"));
        assert!(names.contains(&"add"));
        assert!(names.contains(&"__init__"));
    }

    #[test]
    fn python_slice_function() {
        let a = PythonLanguageAdapter::new();
        let sliced = a.slice_symbol(SRC, "list_tasks").expect("slice");
        assert!(sliced.starts_with("def list_tasks(db):"));
        assert!(sliced.contains("return [r.title for r in rows]"));
        assert!(!sliced.contains("class TaskRepo"));
    }

    #[test]
    fn python_slice_async() {
        let a = PythonLanguageAdapter::new();
        let sliced = a.slice_symbol(SRC, "refresh").expect("slice async");
        assert!(sliced.starts_with("async def refresh():"));
        assert!(sliced.contains("await fetch_once()"));
    }

    #[test]
    fn python_detect_target_from_hint() {
        let a = PythonLanguageAdapter::new();
        let tgt = a.detect_target_symbol(SRC, Some("Fix the bug in list_tasks"));
        assert_eq!(tgt.as_deref(), Some("list_tasks"));
    }

    #[test]
    fn python_detect_target_single_function() {
        let a = PythonLanguageAdapter::new();
        let src = "def only_one(x):\n    return x + 1\n";
        let tgt = a.detect_target_symbol(src, None);
        assert_eq!(tgt.as_deref(), Some("only_one"));
    }

    #[test]
    fn python_extract_symbol_references() {
        let a = PythonLanguageAdapter::new();
        let refs = a.extract_symbol_references(
            "Please fix `list_tasks` so it calls parse_row(r) properly and not the one",
        );
        assert!(refs.contains(&"list_tasks".to_string()));
        assert!(refs.contains(&"parse_row".to_string()));
    }

    #[test]
    fn python_detect_language() {
        let a = PythonLanguageAdapter::new();
        assert!(a.detect_language("main.py"));
        assert!(a.detect_language("pkg/types.pyi"));
        assert!(!a.detect_language("main.go"));
    }

    #[test]
    fn adapter_for_filename_picks_python() {
        let a = adapter_for_filename("app/main.py").expect("adapter");
        assert_eq!(a.name(), "python");
        assert!(adapter_for_filename("app/main.rs").is_none());
    }

    #[test]
    fn slice_includes_decorators() {
        let src = "@cached\n@staticmethod\ndef memo(x):\n    return x\n";
        let a = PythonLanguageAdapter::new();
        let s = a.slice_symbol(src, "memo").expect("slice");
        assert!(s.starts_with("@cached"));
        assert!(s.contains("def memo(x):"));
    }

    #[test]
    fn stopwords_rejected() {
        let a = PythonLanguageAdapter::new();
        let refs = a.extract_symbol_references("The class that returns true if x is None");
        assert!(!refs.iter().any(|r| r == "class"));
        assert!(!refs.iter().any(|r| r == "None"));
        assert!(!refs.iter().any(|r| r == "True"));
    }
}
