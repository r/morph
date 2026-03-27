//! `.morphignore` handling: exclude paths from status and add (same semantics as .gitignore).

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::Path;

/// Build a matcher from `.morphignore` at the repo root, if the file exists.
/// Patterns are interpreted relative to `repo_root` (same as .gitignore).
pub fn load_morphignore(repo_root: &Path) -> Option<Gitignore> {
    let path = repo_root.join(".morphignore");
    if !path.is_file() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(repo_root);
    if builder.add(&path).is_some() {
        return None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_returns_none_when_no_morphignore() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_morphignore(dir.path()).is_none());
    }

    #[test]
    fn load_returns_matcher_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "*.log\n").unwrap();
        assert!(load_morphignore(dir.path()).is_some());
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
        let matcher = load_morphignore(dir.path()).unwrap();
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("debug.log"), false));
        assert!(!is_ignored(Some(&matcher), dir.path(), &dir.path().join("readme.md"), false));
    }

    #[test]
    fn directory_pattern_matches() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "build/\n").unwrap();
        let matcher = load_morphignore(dir.path()).unwrap();
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("build"), true));
        assert!(!is_ignored(Some(&matcher), dir.path(), &dir.path().join("build"), false));
    }

    #[test]
    fn negation_pattern_unignores() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "*.log\n!important.log\n").unwrap();
        let matcher = load_morphignore(dir.path()).unwrap();
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("debug.log"), false));
        assert!(!is_ignored(Some(&matcher), dir.path(), &dir.path().join("important.log"), false));
    }

    #[test]
    fn nested_path_matching() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "logs/*.log\n").unwrap();
        let matcher = load_morphignore(dir.path()).unwrap();
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("logs/app.log"), false));
        assert!(!is_ignored(Some(&matcher), dir.path(), &dir.path().join("app.log"), false));
    }

    #[test]
    fn path_outside_repo_root_not_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "*.log\n").unwrap();
        let matcher = load_morphignore(dir.path()).unwrap();
        let outside = Path::new("/tmp/somewhere/else/debug.log");
        assert!(!is_ignored(Some(&matcher), dir.path(), outside, false));
    }

    #[test]
    fn multiple_patterns() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".morphignore"), "*.log\n*.tmp\ntarget/\n").unwrap();
        let matcher = load_morphignore(dir.path()).unwrap();
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("a.log"), false));
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("b.tmp"), false));
        assert!(is_ignored(Some(&matcher), dir.path(), &dir.path().join("target"), true));
        assert!(!is_ignored(Some(&matcher), dir.path(), &dir.path().join("src/main.rs"), false));
    }
}
