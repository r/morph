use glob::glob;
use serde::Deserialize;
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::Path;

// ── YAML schema ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TestSpec {
    name: String,
    #[serde(default)]
    files: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    dirs: Vec<String>,
    #[serde(default)]
    init: Option<bool>,
    #[serde(default)]
    steps: Vec<Step>,
    #[serde(default, rename = "assert")]
    assertions: Vec<Assertion>,
}

#[derive(Debug, Deserialize)]
struct DeleteFile {
    path: String,
}

#[derive(Debug, Deserialize)]
struct Step {
    #[serde(default)]
    morph: Option<Vec<String>>,
    #[serde(default)]
    compute_hash: Option<ComputeHash>,
    #[serde(default)]
    write_file: Option<WriteFile>,
    #[serde(default)]
    delete_file: Option<DeleteFile>,
    /// Override working directory for this step (relative to temp dir).
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    expect_exit: Option<i32>,
    #[serde(default)]
    capture: Option<String>,
    #[serde(default)]
    capture_first_line: Option<String>,
    /// Parse stdout as JSON and extract a field into a variable.
    #[serde(default)]
    capture_json_field: Option<CaptureJsonField>,
    #[serde(default)]
    assert_hash: Option<bool>,
    #[serde(default)]
    stdout_contains: Option<StringOrVec>,
    #[serde(default)]
    stdout_not_contains: Option<StringOrVec>,
    #[serde(default)]
    stderr_contains: Option<StringOrVec>,
    #[serde(default)]
    assert_hash_length: Option<String>,
    #[serde(default)]
    assert_line_count_gte: Option<AssertLineCount>,
}

#[derive(Debug, Deserialize)]
struct ComputeHash {
    var: String,
    json: String,
}

#[derive(Debug, Deserialize)]
struct CaptureJsonField {
    var: String,
    field: String,
}

#[derive(Debug, Deserialize)]
struct WriteFile {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AssertLineCount {
    var: String,
    min: usize,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringOrVec {
    One(String),
    Many(Vec<String>),
}

impl StringOrVec {
    fn items(&self) -> Vec<&str> {
        match self {
            StringOrVec::One(s) => vec![s.as_str()],
            StringOrVec::Many(v) => v.iter().map(|s| s.as_str()).collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
enum Assertion {
    #[serde(rename = "dir_exists")]
    DirExists { path: String },
    #[serde(rename = "dir_not_exists")]
    DirNotExists { path: String },
    #[serde(rename = "dir_not_empty")]
    DirNotEmpty { path: String },
    #[serde(rename = "file_exists")]
    FileExists { path: String },
    #[serde(rename = "file_not_exists")]
    FileNotExists { path: String },
    #[serde(rename = "file_eq")]
    FileEq { path: String, content: String },
}

// ── codegen ──────────────────────────────────────────────────────────

fn emit_test(spec: &TestSpec) -> String {
    let mut code = String::new();
    let fn_name = spec.name.replace('-', "_");
    let do_init = spec.init.unwrap_or(true);

    writeln!(code, "#[test]").unwrap();
    writeln!(code, "#[allow(unused_variables)]").unwrap();
    writeln!(code, "fn spec_{}() {{", fn_name).unwrap();
    writeln!(code, "    let dir = tempfile::tempdir().unwrap();").unwrap();
    writeln!(code, "    let path = dir.path();").unwrap();
    writeln!(code, "    #[allow(unused_variables)]").unwrap();
    writeln!(code, "    let repo = path.display();").unwrap();

    if do_init {
        writeln!(code, "    {{").unwrap();
        writeln!(
            code,
            "        let mut cmd = cargo_bin_cmd!(\"morph\");"
        )
        .unwrap();
        writeln!(code, "        cmd.arg(\"init\").arg(path).assert().success();").unwrap();
        writeln!(code, "    }}").unwrap();
    }

    for d in &spec.dirs {
        writeln!(
            code,
            "    std::fs::create_dir_all(path.join({:?})).unwrap();",
            d
        )
        .unwrap();
    }

    for (fpath, content) in &spec.files {
        let parent = Path::new(fpath).parent();
        if let Some(p) = parent {
            if p != Path::new("") {
                writeln!(
                    code,
                    "    std::fs::create_dir_all(path.join({:?})).unwrap();",
                    p.to_str().unwrap()
                )
                .unwrap();
            }
        }
        writeln!(
            code,
            "    std::fs::write(path.join({:?}), {:?}).unwrap();",
            fpath, content
        )
        .unwrap();
    }

    for (i, step) in spec.steps.iter().enumerate() {
        emit_step(&mut code, step, i);
    }

    for a in &spec.assertions {
        emit_assertion(&mut code, a);
    }

    writeln!(code, "}}").unwrap();
    code
}

fn emit_step(code: &mut String, step: &Step, idx: usize) {
    if let Some(ch) = &step.compute_hash {
        writeln!(code, "    let {} = {{", ch.var).unwrap();
        writeln!(
            code,
            "        let obj: morph_core::MorphObject = serde_json::from_str({:?}).unwrap();",
            ch.json
        )
        .unwrap();
        writeln!(code, "        morph_core::content_hash(&obj).unwrap()").unwrap();
        writeln!(code, "    }};").unwrap();
        return;
    }

    if let Some(wf) = &step.write_file {
        if wf.content.contains("${") {
            let fmt_str = escape_braces_for_format(&wf.content);
            writeln!(
                code,
                "    std::fs::write(path.join({:?}), format!({:?})).unwrap();",
                wf.path, fmt_str
            )
            .unwrap();
        } else {
            writeln!(
                code,
                "    std::fs::write(path.join({:?}), {:?}).unwrap();",
                wf.path, wf.content
            )
            .unwrap();
        }
        return;
    }

    if let Some(df) = &step.delete_file {
        writeln!(
            code,
            "    std::fs::remove_file(path.join({:?})).unwrap();",
            df.path
        )
        .unwrap();
        return;
    }

    let args = step.morph.as_ref().expect("step must have morph, compute_hash, write_file, or delete_file");
    let needs_output = step.capture.is_some()
        || step.capture_first_line.is_some()
        || step.capture_json_field.is_some()
        || step.assert_line_count_gte.is_some();
    let var = format!("step_{}", idx);

    // Build the command
    writeln!(code, "    let {} = {{", var).unwrap();
    writeln!(
        code,
        "        let mut cmd = cargo_bin_cmd!(\"morph\");"
    )
    .unwrap();
    if let Some(ref dir) = step.cwd {
        if dir.contains("${") {
            let fmt_str = escape_braces_for_format(dir);
            writeln!(code, "        cmd.current_dir(path.join(format!({:?})));", fmt_str).unwrap();
        } else {
            writeln!(code, "        cmd.current_dir(path.join({:?}));", dir).unwrap();
        }
    } else {
        writeln!(code, "        cmd.current_dir(path);").unwrap();
    }

    for arg in args {
        if arg.contains("${") {
            let fmt_str = escape_braces_for_format(arg);
            writeln!(code, "        cmd.arg(format!({:?}));", fmt_str).unwrap();
        } else {
            writeln!(code, "        cmd.arg({:?});", arg).unwrap();
        }
    }

    // Build the assertion chain: exit code + stdout/stderr predicates
    let expect_success = step.expect_exit.is_none_or(|c| c == 0);
    if expect_success {
        write!(code, "        cmd.assert().success()").unwrap();
    } else {
        write!(
            code,
            "        cmd.assert().code(predicates::ord::eq({}))",
            step.expect_exit.unwrap()
        )
        .unwrap();
    }

    if let Some(ref contains) = step.stdout_contains {
        for s in contains.items() {
            if s.contains("${") {
                let fmt_str = escape_braces_for_format(s);
                write!(
                    code,
                    "\n            .stdout(predicates::prelude::predicate::str::contains(format!({fmt_str:?})))",
                )
                .unwrap();
            } else {
                write!(
                    code,
                    "\n            .stdout(predicates::prelude::predicate::str::contains({s:?}))",
                )
                .unwrap();
            }
        }
    }
    if let Some(ref not_contains) = step.stdout_not_contains {
        for s in not_contains.items() {
            if s.contains("${") {
                let fmt_str = escape_braces_for_format(s);
                write!(
                    code,
                    "\n            .stdout(predicates::prelude::predicate::str::contains(format!({fmt_str:?})).not())",
                )
                .unwrap();
            } else {
                write!(
                    code,
                    "\n            .stdout(predicates::prelude::predicate::str::contains({s:?}).not())",
                )
                .unwrap();
            }
        }
    }
    if let Some(ref contains) = step.stderr_contains {
        for s in contains.items() {
            if s.contains("${") {
                let fmt_str = escape_braces_for_format(s);
                write!(
                    code,
                    "\n            .stderr(predicates::prelude::predicate::str::contains(format!({fmt_str:?})))",
                )
                .unwrap();
            } else {
                write!(
                    code,
                    "\n            .stderr(predicates::prelude::predicate::str::contains({s:?}))",
                )
                .unwrap();
            }
        }
    }
    writeln!(code).unwrap();

    writeln!(code, "    }};").unwrap();

    // Capture output into variables
    if let Some(var_name) = &step.capture {
        writeln!(
            code,
            "    let {var_name} = String::from_utf8_lossy(&{var}.get_output().stdout).trim().to_string();"
        )
        .unwrap();
    }

    if let Some(var_name) = &step.capture_first_line {
        writeln!(
            code,
            "    let {var_name} = String::from_utf8_lossy(&{var}.get_output().stdout).trim().lines().next().unwrap_or_default().to_string();"
        )
        .unwrap();
    }

    if let Some(cjf) = &step.capture_json_field {
        let var_name = &cjf.var;
        let field = &cjf.field;
        writeln!(
            code,
            "    let {var_name} = {{ let __json: serde_json::Value = serde_json::from_str(String::from_utf8_lossy(&{var}.get_output().stdout).trim()).expect(\"stdout is not valid JSON\"); __json[{field:?}].as_str().expect(\"JSON field '{field}' missing or not a string\").to_string() }};"
        )
        .unwrap();
    }

    // Post-step assertions on captured values
    if step.assert_hash == Some(true) {
        let cap = step
            .capture
            .as_ref()
            .or(step.capture_first_line.as_ref())
            .or(step.capture_json_field.as_ref().map(|c| &c.var))
            .expect("assert_hash requires capture");
        writeln!(
            code,
            "    assert_eq!({cap}.len(), 64, \"expected 64-char hash, got '{{}}'\", {cap});"
        )
        .unwrap();
    }

    if let Some(ref var_name) = step.assert_hash_length {
        writeln!(
            code,
            "    assert_eq!({var_name}.len(), 64, \"expected 64-char hash, got '{{}}'\", {var_name});"
        )
        .unwrap();
    }

    if let Some(ref alc) = step.assert_line_count_gte {
        writeln!(
            code,
            "    {{ let lines: Vec<_> = {}.trim().lines().collect(); assert!(lines.len() >= {}, \"expected >= {} lines, got {{}}\", lines.len()); }}",
            alc.var, alc.min, alc.min
        )
        .unwrap();
    }

    let _ = needs_output;
}

fn emit_assertion(code: &mut String, a: &Assertion) {
    match a {
        Assertion::DirExists { path } => {
            writeln!(
                code,
                "    assert!(path.join({:?}).is_dir(), \"{} should be a directory\");",
                path, path
            )
            .unwrap();
        }
        Assertion::DirNotExists { path } => {
            writeln!(
                code,
                "    assert!(!path.join({:?}).exists(), \"{} should not exist\");",
                path, path
            )
            .unwrap();
        }
        Assertion::DirNotEmpty { path } => {
            writeln!(
                code,
                "    {{ let p = path.join({:?}); assert!(p.is_dir(), \"{} should be a directory\"); assert!(!std::fs::read_dir(&p).unwrap().next().is_none(), \"{} should not be empty\"); }}",
                path, path, path
            )
            .unwrap();
        }
        Assertion::FileExists { path } => {
            writeln!(
                code,
                "    assert!(path.join({:?}).is_file(), \"{} should be a file\");",
                path, path
            )
            .unwrap();
        }
        Assertion::FileNotExists { path } => {
            writeln!(
                code,
                "    assert!(!path.join({:?}).exists(), \"{} should not exist\");",
                path, path
            )
            .unwrap();
        }
        Assertion::FileEq { path, content } => {
            writeln!(
                code,
                "    assert_eq!(std::fs::read_to_string(path.join({:?})).unwrap(), {:?});",
                path, content
            )
            .unwrap();
        }
    }
}

/// Escapes `{` and `}` for use in `format!()`, then converts `${var}` into `{var}`.
/// This ensures JSON braces become `{{`/`}}` while variable refs become format args.
fn escape_braces_for_format(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
            // ${var_name} -> {var_name}
            result.push('{');
            i += 2; // skip ${ 
            while i < chars.len() && chars[i] != '}' {
                result.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                result.push('}'); // closing brace
                i += 1;
            }
        } else if chars[i] == '{' {
            result.push_str("{{");
            i += 1;
        } else if chars[i] == '}' {
            result.push_str("}}");
            i += 1;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

// ── main ─────────────────────────────────────────────────────────────

fn main() {
    // Force build.rs to re-run every time so the timestamp is always fresh.
    println!("cargo:rerun-if-changed=_always_rebuild");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    println!("cargo:rustc-env=MORPH_BUILD_DATE={}", epoch_to_iso(now));

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let specs_dir = Path::new(&manifest_dir).join("tests/specs");

    if !specs_dir.is_dir() {
        return;
    }

    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join("spec_tests.rs");

    let mut all_code = String::from(
        "// Auto-generated from tests/specs/*.yaml -- do not edit.\n\
         use assert_cmd::cargo::cargo_bin_cmd;\n\
         #[allow(unused_imports)]\n\
         use predicates::prelude::*;\n\n",
    );

    let pattern = specs_dir.join("*.yaml").to_str().unwrap().to_string();
    let mut spec_files: Vec<_> = glob(&pattern).unwrap().filter_map(Result::ok).collect();
    spec_files.sort();

    for entry in spec_files {
        let yaml = fs::read_to_string(&entry).expect("cannot read spec file");
        let specs: Vec<TestSpec> =
            serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("bad YAML in {}: {}", entry.display(), e));

        for spec in &specs {
            all_code.push_str(&emit_test(spec));
            all_code.push('\n');
        }
    }

    fs::write(&out_path, all_code).unwrap();
}

fn epoch_to_iso(secs: u64) -> String {
    let days = secs / 86400;
    let day_secs = secs % 86400;
    let hh = day_secs / 3600;
    let mm = (day_secs % 3600) / 60;
    let ss = day_secs % 60;

    let mut y: u64 = 1970;
    let mut rem = days;
    loop {
        let ylen = if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) { 366 } else { 365 };
        if rem < ylen { break; }
        rem -= ylen;
        y += 1;
    }
    let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut mo = 0u64;
    for md in mdays {
        if rem < md { break; }
        rem -= md;
        mo += 1;
    }
    format!("{y:04}-{:02}-{:02}T{hh:02}:{mm:02}:{ss:02}Z", mo + 1, rem + 1)
}
