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
