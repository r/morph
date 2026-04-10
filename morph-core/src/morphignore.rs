//! Ignore-rule handling for Morph: built-in defaults, `.gitignore`, and `.morphignore`.
//!
//! Rules are layered (later layers override earlier ones via gitignore negation semantics):
//! 1. Hardcoded built-in defaults (VCS dirs, caches, virtual envs, build artifacts)
//! 2. `.gitignore` at repo root (if present)
//! 3. `.morphignore` at repo root (if present — can negate defaults with `!pattern`)

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::Path;

const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    // VCS internals
    ".git/",
    ".hg/",
    ".svn/",
    // Dependencies
    "node_modules/",
    // Python
    "__pycache__/",
    "*.pyc",
    "*.pyo",
    "*.egg-info/",
    // Virtual environments
    ".venv/",
    "venv/",
    ".env/",
    // Build artifacts
    "target/",
    "dist/",
    "build/",
    "*.so",
    "*.dylib",
    // Caches
    ".mypy_cache/",
    ".pytest_cache/",
    ".ruff_cache/",
    ".tox/",
    ".nox/",
    ".cache/",
    // OS files
    ".DS_Store",
    "Thumbs.db",
    // Editor swap files
    "*.swp",
    "*.swo",
];

/// Build a combined ignore matcher from built-in defaults, `.gitignore`, and `.morphignore`.
/// Always returns `Some` because the built-in defaults are always present.
pub fn load_ignore_rules(repo_root: &Path) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(repo_root);

    for pattern in DEFAULT_IGNORE_PATTERNS {
        let _ = builder.add_line(None, pattern);
    }

    let gitignore = repo_root.join(".gitignore");
    if gitignore.is_file() {
        let _ = builder.add(&gitignore);
    }

    let morphignore = repo_root.join(".morphignore");
    if morphignore.is_file() {
        let _ = builder.add(&morphignore);
    }

    builder.build().ok()
}

/// Returns true if `path` should be ignored. `path` must be absolute and under `repo_root`;
/// it is converted to a path relative to `repo_root` for matching.
pub fn is_ignored(
    matcher: Option<&Gitignore>,
    repo_root: &Path,
    path: &Path,
    is_dir: bool,
) -> bool {
    let Some(m) = matcher else {
        return false;
    };
    let relative = match path.strip_prefix(repo_root) {
        Ok(r) => r,
        Err(_) => return false,
    };
    matches!(m.matched(relative, is_dir), ignore::Match::Ignore(_))
}

/// Check if a relative path (as stored in the index or tree) should be ignored.
/// Also checks parent directory components, so `.git/config` is caught by the `.git/` pattern.
pub fn is_rel_path_ignored(
    matcher: Option<&Gitignore>,
    relative_path: &str,
    is_dir: bool,
) -> bool {
    let Some(m) = matcher else {
        return false;
    };
    let p = Path::new(relative_path);
    if matches!(m.matched(p, is_dir), ignore::Match::Ignore(_)) {
        return true;
    }
    // Walk parent components: if any ancestor directory is ignored, the file is too.
    let mut ancestor = p.parent();
    while let Some(dir) = ancestor {
        if dir.as_os_str().is_empty() {
            break;
        }
        if matches!(m.matched(dir, true), ignore::Match::Ignore(_)) {
            return true;
        }
        ancestor = dir.parent();
    }
    false
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_returns_some_even_without_any_ignore_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_ignore_rules(dir.path()).is_some());
    }

    #[test]
    fn builtin_defaults_ignore_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join(".git"),
            true,
        ));
        // Files inside .git/ are handled by walkdir pruning the directory entry,
        // so is_ignored on a file inside .git/ won't match the dir-only pattern.
        // Use is_rel_path_ignored for index/tree filtering which walks parent dirs.
        assert!(is_rel_path_ignored(Some(&matcher), ".git/config", false));
    }

    #[test]
    fn builtin_defaults_ignore_node_modules() {
        let dir = tempfile::tempdir().unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("node_modules"),
            true,
        ));
    }

    #[test]
    fn builtin_defaults_ignore_pycache() {
        let dir = tempfile::tempdir().unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("__pycache__"),
            true,
        ));
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("foo.pyc"),
            false,
        ));
    }

    #[test]
    fn builtin_defaults_ignore_venv() {
        let dir = tempfile::tempdir().unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join(".venv"),
            true,
        ));
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("venv"),
            true,
        ));
    }

    #[test]
    fn builtin_defaults_ignore_target() {
        let dir = tempfile::tempdir().unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("target"),
            true,
        ));
    }

    #[test]
    fn builtin_defaults_do_not_ignore_source_files() {
        let dir = tempfile::tempdir().unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(!is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("src/main.rs"),
            false,
        ));
        assert!(!is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("README.md"),
            false,
        ));
        assert!(!is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("pyproject.toml"),
            false,
        ));
    }

    #[test]
    fn gitignore_patterns_are_respected() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "secret.key\nlogs/\n").unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("secret.key"),
            false,
        ));
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("logs"),
            true,
        ));
        // Source files still not ignored
        assert!(!is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("app.py"),
            false,
        ));
    }

    #[test]
    fn morphignore_can_negate_builtin_defaults() {
        let dir = tempfile::tempdir().unwrap();
        // Un-ignore the target/ directory via .morphignore negation
        fs::write(dir.path().join(".morphignore"), "!target/\n").unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(
            !is_ignored(Some(&matcher), dir.path(), &dir.path().join("target"), true),
            "target/ should be un-ignored by .morphignore negation"
        );
        // Other defaults still apply
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join(".git"),
            true,
        ));
    }

    #[test]
    fn morphignore_adds_extra_patterns() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "*.log\n").unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(
            Some(&matcher),
            dir.path(),
            &dir.path().join("debug.log"),
            false,
        ));
    }

    #[test]
    fn is_rel_path_ignored_works_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_rel_path_ignored(Some(&matcher), ".git/config", false));
        assert!(is_rel_path_ignored(Some(&matcher), ".venv/bin/python", false));
        assert!(is_rel_path_ignored(Some(&matcher), "node_modules/foo/index.js", false));
        assert!(!is_rel_path_ignored(Some(&matcher), "src/main.rs", false));
        assert!(!is_rel_path_ignored(Some(&matcher), "README.md", false));
    }

    #[test]
    fn is_rel_path_ignored_returns_false_when_no_matcher() {
        assert!(!is_rel_path_ignored(None, ".git/config", false));
    }

    // Legacy tests (updated to use new function name)

    #[test]
    fn load_returns_matcher_when_morphignore_exists() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "*.log\n").unwrap();
        assert!(load_ignore_rules(dir.path()).is_some());
    }

    #[test]
    fn is_ignored_returns_false_when_no_matcher() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("foo.txt");
        assert!(!is_ignored(None, dir.path(), &p, false));
    }

    #[test]
    fn simple_glob_pattern_matches() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "*.log\n").unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("debug.log"), false));
        assert!(!is_ignored(Some(&matcher), dir.path(), &dir.path().join("readme.md"), false));
    }

    #[test]
    fn directory_pattern_matches() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "vendor/\n").unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("vendor"), true));
        assert!(!is_ignored(Some(&matcher), dir.path(), &dir.path().join("vendor"), false));
    }

    #[test]
    fn negation_pattern_unignores() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "*.log\n!important.log\n").unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("debug.log"), false));
        assert!(!is_ignored(Some(&matcher), dir.path(), &dir.path().join("important.log"), false));
    }

    #[test]
    fn nested_path_matching() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "logs/*.log\n").unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("logs/app.log"), false));
        assert!(!is_ignored(Some(&matcher), dir.path(), &dir.path().join("app.log"), false));
    }

    #[test]
    fn path_outside_repo_root_not_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "*.log\n").unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        let outside = Path::new("/tmp/somewhere/else/debug.log");
        assert!(!is_ignored(Some(&matcher), dir.path(), outside, false));
    }

    #[test]
    fn multiple_patterns() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "*.log\n*.tmp\nvendor/\n").unwrap();
        let matcher = load_ignore_rules(dir.path()).unwrap();
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("a.log"), false));
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("b.tmp"), false));
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("vendor"), true));
        assert!(!is_ignored(Some(&matcher), dir.path(), &dir.path().join("src/main.rs"), false));
    }
}
