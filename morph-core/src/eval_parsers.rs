//! Phase 3a: parse common test-runner stdout into the canonical
//! Morph metric map (`{tests_total, tests_passed, tests_failed, ...}`).
//!
//! The rules of the road:
//!
//! - Be lenient. Parsers see real-world output that may be truncated,
//!   mixed with logs, or contain ANSI escapes. We accept what we can
//!   match and skip what we can't.
//! - Aggregate across multi-binary / multi-suite runs. `cargo test`
//!   prints one summary line per binary; we sum them.
//! - Only emit metrics we can vouch for. If we can't determine
//!   `pass_rate`, we don't fabricate it.
//!
//! Returned maps use `f64` so they plug straight into
//! `EvalContract.observed_metrics` without further conversion.

use std::collections::BTreeMap;

/// Strip ANSI color escape sequences. Lots of CI output is colored,
/// and our regexes are simpler if we kill those up front.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2;
            while i < bytes.len()
                && !(bytes[i] >= 0x40 && bytes[i] <= 0x7e)
            {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn pass_rate(passed: u64, total: u64) -> Option<f64> {
    if total == 0 {
        None
    } else {
        Some(passed as f64 / total as f64)
    }
}

/// Parse `cargo test` stdout. Each test binary emits a line like:
///
/// ```text
/// test result: ok. 42 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.12s
/// ```
///
/// We sum across every such line (multi-binary workspaces produce
/// one per crate). Returns `tests_passed`, `tests_failed`,
/// `tests_ignored`, `tests_measured`, `tests_total`, `pass_rate`,
/// and `wall_time_secs` (the sum of all `finished in`).
pub fn parse_cargo_test(stdout: &str) -> BTreeMap<String, f64> {
    let cleaned = strip_ansi(stdout);
    let mut passed: u64 = 0;
    let mut failed: u64 = 0;
    let mut ignored: u64 = 0;
    let mut measured: u64 = 0;
    let mut wall: f64 = 0.0;
    let mut found_any = false;

    for line in cleaned.lines() {
        let l = line.trim();
        if !l.starts_with("test result:") {
            continue;
        }
        found_any = true;
        passed += extract_num_before(l, " passed").unwrap_or(0);
        failed += extract_num_before(l, " failed").unwrap_or(0);
        ignored += extract_num_before(l, " ignored").unwrap_or(0);
        measured += extract_num_before(l, " measured").unwrap_or(0);
        if let Some(secs) = extract_finished_in_secs(l) {
            wall += secs;
        }
    }

    let mut out = BTreeMap::new();
    if !found_any {
        return out;
    }
    let total = passed + failed + ignored;
    out.insert("tests_passed".into(), passed as f64);
    out.insert("tests_failed".into(), failed as f64);
    out.insert("tests_ignored".into(), ignored as f64);
    if measured > 0 {
        out.insert("tests_measured".into(), measured as f64);
    }
    out.insert("tests_total".into(), total as f64);
    if let Some(r) = pass_rate(passed, total) {
        out.insert("pass_rate".into(), r);
    }
    if wall > 0.0 {
        out.insert("wall_time_secs".into(), wall);
    }
    out
}

/// Parse `pytest` stdout. The terminal summary looks like:
///
/// ```text
/// === 12 passed, 1 failed, 2 skipped in 3.45s ===
/// ```
///
/// pytest is quite consistent about emitting one terminal summary,
/// so we don't aggregate. We also support the older single-counter
/// shape (`=== 5 passed in 0.10s ===`).
pub fn parse_pytest(stdout: &str) -> BTreeMap<String, f64> {
    let cleaned = strip_ansi(stdout);
    let mut passed: u64 = 0;
    let mut failed: u64 = 0;
    let mut skipped: u64 = 0;
    let mut errors: u64 = 0;
    let mut wall: f64 = 0.0;
    let mut found_any = false;

    for line in cleaned.lines() {
        let l = line.trim();
        if !l.starts_with("===") || !l.contains(" in ") {
            continue;
        }
        if !(l.contains(" passed")
            || l.contains(" failed")
            || l.contains(" error")
            || l.contains(" skipped"))
        {
            continue;
        }
        found_any = true;
        passed = extract_num_before(l, " passed").unwrap_or(passed);
        failed = extract_num_before(l, " failed").unwrap_or(failed);
        skipped = extract_num_before(l, " skipped").unwrap_or(skipped);
        errors = extract_num_before(l, " error").unwrap_or(errors);
        if let Some(secs) = extract_pytest_seconds(l) {
            wall = secs;
        }
    }

    let mut out = BTreeMap::new();
    if !found_any {
        return out;
    }
    let total = passed + failed + skipped + errors;
    out.insert("tests_passed".into(), passed as f64);
    out.insert("tests_failed".into(), failed as f64);
    if skipped > 0 {
        out.insert("tests_skipped".into(), skipped as f64);
    }
    if errors > 0 {
        out.insert("tests_errors".into(), errors as f64);
    }
    out.insert("tests_total".into(), total as f64);
    if let Some(r) = pass_rate(passed, total) {
        out.insert("pass_rate".into(), r);
    }
    if wall > 0.0 {
        out.insert("wall_time_secs".into(), wall);
    }
    out
}

/// Parse `vitest` stdout. Vitest writes a `Tests` summary block:
///
/// ```text
/// Test Files  3 passed (3)
///      Tests  42 passed | 1 failed | 2 skipped (45)
///   Start at  10:23:45
///   Duration  1.23s
/// ```
pub fn parse_vitest(stdout: &str) -> BTreeMap<String, f64> {
    let cleaned = strip_ansi(stdout);
    let mut passed: u64 = 0;
    let mut failed: u64 = 0;
    let mut skipped: u64 = 0;
    let mut total: u64 = 0;
    let mut wall: f64 = 0.0;
    let mut found_any = false;

    for line in cleaned.lines() {
        let l = line.trim();
        if l.starts_with("Tests") && l.contains("passed") {
            found_any = true;
            passed = extract_num_before(l, " passed").unwrap_or(0);
            failed = extract_num_before(l, " failed").unwrap_or(0);
            skipped = extract_num_before(l, " skipped").unwrap_or(0);
            if let Some(t) = extract_paren_total(l) {
                total = t;
            }
        } else if l.starts_with("Duration") {
            if let Some(secs) = extract_duration_secs(l) {
                wall = secs;
            }
        }
    }

    let mut out = BTreeMap::new();
    if !found_any {
        return out;
    }
    if total == 0 {
        total = passed + failed + skipped;
    }
    out.insert("tests_passed".into(), passed as f64);
    out.insert("tests_failed".into(), failed as f64);
    if skipped > 0 {
        out.insert("tests_skipped".into(), skipped as f64);
    }
    out.insert("tests_total".into(), total as f64);
    if let Some(r) = pass_rate(passed, total) {
        out.insert("pass_rate".into(), r);
    }
    if wall > 0.0 {
        out.insert("wall_time_secs".into(), wall);
    }
    out
}

/// Parse `jest` stdout. Jest's summary is a multi-line block:
///
/// ```text
/// Tests:       1 failed, 41 passed, 42 total
/// Snapshots:   0 total
/// Time:        2.345 s
/// ```
pub fn parse_jest(stdout: &str) -> BTreeMap<String, f64> {
    let cleaned = strip_ansi(stdout);
    let mut passed: u64 = 0;
    let mut failed: u64 = 0;
    let mut skipped: u64 = 0;
    let mut total: u64 = 0;
    let mut wall: f64 = 0.0;
    let mut found_any = false;

    for line in cleaned.lines() {
        let l = line.trim();
        if l.starts_with("Tests:") {
            found_any = true;
            passed = extract_num_before(l, " passed").unwrap_or(0);
            failed = extract_num_before(l, " failed").unwrap_or(0);
            skipped = extract_num_before(l, " skipped").unwrap_or(0);
            total = extract_num_before(l, " total").unwrap_or(passed + failed + skipped);
        } else if l.starts_with("Time:") {
            if let Some(secs) = extract_jest_time_secs(l) {
                wall = secs;
            }
        }
    }

    let mut out = BTreeMap::new();
    if !found_any {
        return out;
    }
    out.insert("tests_passed".into(), passed as f64);
    out.insert("tests_failed".into(), failed as f64);
    if skipped > 0 {
        out.insert("tests_skipped".into(), skipped as f64);
    }
    out.insert("tests_total".into(), total as f64);
    if let Some(r) = pass_rate(passed, total) {
        out.insert("pass_rate".into(), r);
    }
    if wall > 0.0 {
        out.insert("wall_time_secs".into(), wall);
    }
    out
}

/// Parse `go test` stdout. Go is irritatingly inconsistent — package
/// summaries look like `ok   foo  0.123s` or `FAIL   foo  0.456s`, and
/// individual test outcomes have `--- PASS: TestX (0.00s)` / `--- FAIL`
/// / `--- SKIP`. We sum across all package and test lines.
pub fn parse_go_test(stdout: &str) -> BTreeMap<String, f64> {
    let cleaned = strip_ansi(stdout);
    let mut passed: u64 = 0;
    let mut failed: u64 = 0;
    let mut skipped: u64 = 0;
    let mut wall: f64 = 0.0;
    let mut found_any = false;

    for line in cleaned.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("--- PASS") {
            passed += 1;
            wall += extract_paren_secs(rest).unwrap_or(0.0);
            found_any = true;
        } else if let Some(rest) = l.strip_prefix("--- FAIL") {
            failed += 1;
            wall += extract_paren_secs(rest).unwrap_or(0.0);
            found_any = true;
        } else if l.starts_with("--- SKIP") {
            skipped += 1;
            found_any = true;
        }
    }

    let mut out = BTreeMap::new();
    if !found_any {
        return out;
    }
    let total = passed + failed + skipped;
    out.insert("tests_passed".into(), passed as f64);
    out.insert("tests_failed".into(), failed as f64);
    if skipped > 0 {
        out.insert("tests_skipped".into(), skipped as f64);
    }
    out.insert("tests_total".into(), total as f64);
    if let Some(r) = pass_rate(passed, total) {
        out.insert("pass_rate".into(), r);
    }
    if wall > 0.0 {
        out.insert("wall_time_secs".into(), wall);
    }
    out
}

/// Dispatch to the requested runner-specific parser. The `runner`
/// string is matched case-insensitively against the canonical names
/// (`cargo`, `pytest`, `vitest`, `jest`, `go`); anything else (most
/// notably `auto`) falls back to [`parse_auto`].
///
/// This is the single dispatch point shared by `morph eval from-output`,
/// `morph eval run`, and the matching MCP tools, so adding a new runner
/// only requires one edit.
pub fn parse_with_runner(
    runner: &str,
    output: &str,
    hint: Option<&str>,
) -> BTreeMap<String, f64> {
    match runner.to_lowercase().as_str() {
        "cargo" => parse_cargo_test(output),
        "pytest" => parse_pytest(output),
        "vitest" => parse_vitest(output),
        "jest" => parse_jest(output),
        "go" => parse_go_test(output),
        _ => parse_auto(output, hint),
    }
}

/// Auto-detect the runner from the output (or from a `hint` like the
/// CLI command string the user invoked) and parse accordingly. Falls
/// back to an empty map when no signal is found.
pub fn parse_auto(stdout: &str, hint: Option<&str>) -> BTreeMap<String, f64> {
    if let Some(h) = hint {
        let h = h.trim().to_lowercase();
        if h.starts_with("cargo test") || h.starts_with("cargo nextest") {
            return parse_cargo_test(stdout);
        }
        if h.starts_with("pytest") || h.starts_with("python -m pytest") {
            return parse_pytest(stdout);
        }
        if h.starts_with("vitest") || h.contains("vitest") {
            return parse_vitest(stdout);
        }
        if h.starts_with("jest") || h.contains("jest") {
            return parse_jest(stdout);
        }
        if h.starts_with("go test") {
            return parse_go_test(stdout);
        }
    }
    let cleaned = strip_ansi(stdout);
    if cleaned.contains("test result: ok") || cleaned.contains("test result: FAILED") {
        return parse_cargo_test(stdout);
    }
    if cleaned.lines().any(|l| {
        let t = l.trim();
        t.starts_with("===") && t.contains(" in ") && t.contains("passed")
    }) {
        return parse_pytest(stdout);
    }
    if cleaned.lines().any(|l| l.trim().starts_with("Tests") && l.contains("passed")) {
        if cleaned.lines().any(|l| l.trim().starts_with("Tests:")) {
            return parse_jest(stdout);
        }
        return parse_vitest(stdout);
    }
    if cleaned.contains("--- PASS") || cleaned.contains("--- FAIL") {
        return parse_go_test(stdout);
    }
    BTreeMap::new()
}

// ── helpers ──────────────────────────────────────────────────────────

/// Find the integer literal immediately preceding `marker` in `line`,
/// e.g. `extract_num_before("3 passed", " passed") == Some(3)`.
fn extract_num_before(line: &str, marker: &str) -> Option<u64> {
    let idx = line.find(marker)?;
    let prefix = &line[..idx];
    let mut end = prefix.len();
    while end > 0 && !prefix.as_bytes()[end - 1].is_ascii_digit() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && prefix.as_bytes()[start - 1].is_ascii_digit() {
        start -= 1;
    }
    if start == end {
        return None;
    }
    prefix[start..end].parse().ok()
}

/// `cargo` line → `finished in 0.12s` → `0.12_f64`.
fn extract_finished_in_secs(line: &str) -> Option<f64> {
    let idx = line.find("finished in ")?;
    let rest = &line[idx + "finished in ".len()..];
    parse_seconds_prefix(rest)
}

fn extract_pytest_seconds(line: &str) -> Option<f64> {
    let idx = line.find(" in ")?;
    let rest = &line[idx + " in ".len()..];
    parse_seconds_prefix(rest)
}

fn extract_duration_secs(line: &str) -> Option<f64> {
    let after = line.split_whitespace().nth(1)?;
    parse_seconds_prefix(after)
}

fn extract_jest_time_secs(line: &str) -> Option<f64> {
    let rest = line.trim_start_matches("Time:").trim();
    let mut iter = rest.split_whitespace();
    let num = iter.next()?;
    num.parse::<f64>().ok()
}

fn extract_paren_total(line: &str) -> Option<u64> {
    let start = line.rfind('(')?;
    let end = line[start..].find(')')?;
    line[start + 1..start + end].parse().ok()
}

fn extract_paren_secs(rest: &str) -> Option<f64> {
    let start = rest.find('(')?;
    let end = rest[start..].find(')')?;
    parse_seconds_prefix(&rest[start + 1..start + end])
}

fn parse_seconds_prefix(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    let mut end = 0;
    let bytes = trimmed.as_bytes();
    while end < bytes.len() {
        let b = bytes[end];
        if b.is_ascii_digit() || b == b'.' {
            end += 1;
        } else {
            break;
        }
    }
    if end == 0 {
        return None;
    }
    let num: f64 = trimmed[..end].parse().ok()?;
    let unit = trimmed[end..].trim_start();
    if unit.starts_with("ms") {
        Some(num / 1000.0)
    } else {
        Some(num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_simple_passing_run() {
        let out = "running 3 tests\n\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s\n";
        let m = parse_cargo_test(out);
        assert_eq!(m["tests_passed"], 3.0);
        assert_eq!(m["tests_failed"], 0.0);
        assert_eq!(m["tests_total"], 3.0);
        assert_eq!(m["pass_rate"], 1.0);
        assert!(m.contains_key("wall_time_secs"));
    }

    #[test]
    fn cargo_aggregates_multi_binary() {
        let out = "test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s\n\
                   test result: FAILED. 7 passed; 2 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.20s\n";
        let m = parse_cargo_test(out);
        assert_eq!(m["tests_passed"], 12.0);
        assert_eq!(m["tests_failed"], 2.0);
        assert_eq!(m["tests_ignored"], 1.0);
        assert_eq!(m["tests_total"], 15.0);
        assert!((m["pass_rate"] - 0.8).abs() < 1e-9);
        assert!((m["wall_time_secs"] - 0.30).abs() < 1e-9);
    }

    #[test]
    fn cargo_handles_ansi_escapes() {
        let out = "\u{1b}[32mtest result: ok\u{1b}[0m. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        let m = parse_cargo_test(out);
        assert_eq!(m["tests_passed"], 1.0);
    }

    #[test]
    fn cargo_returns_empty_when_no_summary() {
        let m = parse_cargo_test("compiling foo v1.0.0\nfinished\n");
        assert!(m.is_empty());
    }

    #[test]
    fn pytest_full_summary() {
        let out = "============================= test session starts ==============================\n\
                   ============================== 12 passed, 1 failed, 2 skipped in 3.45s ==============================\n";
        let m = parse_pytest(out);
        assert_eq!(m["tests_passed"], 12.0);
        assert_eq!(m["tests_failed"], 1.0);
        assert_eq!(m["tests_skipped"], 2.0);
        assert_eq!(m["tests_total"], 15.0);
        assert!((m["wall_time_secs"] - 3.45).abs() < 1e-9);
    }

    #[test]
    fn pytest_simple_passing_run() {
        let out = "===== 5 passed in 0.10s =====\n";
        let m = parse_pytest(out);
        assert_eq!(m["tests_passed"], 5.0);
        assert_eq!(m["tests_total"], 5.0);
        assert_eq!(m["pass_rate"], 1.0);
    }

    #[test]
    fn vitest_summary_block() {
        let out = "Test Files  3 passed (3)\n\
                       Tests  42 passed | 1 failed | 2 skipped (45)\n\
                  Start at  10:23:45\n\
                   Duration  1.23s\n";
        let m = parse_vitest(out);
        assert_eq!(m["tests_passed"], 42.0);
        assert_eq!(m["tests_failed"], 1.0);
        assert_eq!(m["tests_skipped"], 2.0);
        assert_eq!(m["tests_total"], 45.0);
        assert!((m["wall_time_secs"] - 1.23).abs() < 1e-9);
    }

    #[test]
    fn jest_summary_block() {
        let out = "Tests:       1 failed, 41 passed, 42 total\n\
                   Snapshots:   0 total\n\
                   Time:        2.345 s\n";
        let m = parse_jest(out);
        assert_eq!(m["tests_passed"], 41.0);
        assert_eq!(m["tests_failed"], 1.0);
        assert_eq!(m["tests_total"], 42.0);
        assert!((m["wall_time_secs"] - 2.345).abs() < 1e-9);
    }

    #[test]
    fn go_test_aggregates_individual_outcomes() {
        let out = "=== RUN   TestA\n\
                   --- PASS: TestA (0.10s)\n\
                   === RUN   TestB\n\
                   --- FAIL: TestB (0.20s)\n\
                   === RUN   TestC\n\
                   --- SKIP: TestC (0.00s)\n\
                   FAIL\nFAIL  example.com/foo  0.30s\n";
        let m = parse_go_test(out);
        assert_eq!(m["tests_passed"], 1.0);
        assert_eq!(m["tests_failed"], 1.0);
        assert_eq!(m["tests_skipped"], 1.0);
        assert_eq!(m["tests_total"], 3.0);
        assert!((m["wall_time_secs"] - 0.30).abs() < 1e-9);
    }

    #[test]
    fn auto_uses_hint_when_available() {
        let out = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        let m = parse_auto(out, Some("cargo test --workspace"));
        assert_eq!(m["tests_passed"], 1.0);
    }

    #[test]
    fn auto_content_sniffs_pytest() {
        let out = "===== 7 passed, 1 failed in 0.50s =====\n";
        let m = parse_auto(out, None);
        assert_eq!(m["tests_passed"], 7.0);
        assert_eq!(m["tests_failed"], 1.0);
    }

    #[test]
    fn auto_distinguishes_jest_from_vitest() {
        let jest = "Tests:       3 passed, 3 total\nTime: 1.0 s\n";
        let m_jest = parse_auto(jest, None);
        assert_eq!(m_jest["tests_passed"], 3.0);
        assert_eq!(m_jest["tests_total"], 3.0);

        let vitest = "Test Files  1 passed (1)\n     Tests  3 passed (3)\n  Duration  1s\n";
        let m_vit = parse_auto(vitest, None);
        assert_eq!(m_vit["tests_passed"], 3.0);
        assert_eq!(m_vit["tests_total"], 3.0);
    }

    #[test]
    fn auto_returns_empty_on_unknown_input() {
        let m = parse_auto("just some random output\n", None);
        assert!(m.is_empty());
    }

    #[test]
    fn parse_with_runner_dispatches_named_runners() {
        let cargo = "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        assert_eq!(parse_with_runner("cargo", cargo, None)["tests_passed"], 2.0);
        assert_eq!(parse_with_runner("CARGO", cargo, None)["tests_passed"], 2.0);

        let pytest = "===== 4 passed in 0.10s =====\n";
        assert_eq!(parse_with_runner("pytest", pytest, None)["tests_passed"], 4.0);
    }

    #[test]
    fn parse_with_runner_unknown_falls_back_to_auto() {
        let cargo = "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        let m = parse_with_runner("auto", cargo, None);
        assert_eq!(m["tests_passed"], 1.0);
        let m2 = parse_with_runner("something-else", cargo, Some("cargo test"));
        assert_eq!(m2["tests_passed"], 1.0);
    }
}
