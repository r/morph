// Auto-generated CLI specs live in `tests/specs/*.yaml` and are
// compiled into a single Rust file by `morph-cli/build.rs`. The
// generator emits some patterns clippy would prefer simplified (e.g.
// `format!("{var}")`, `!iter.next().is_none()`); these come from a
// straightforward template, so we allow the lints at the include site
// rather than carrying a heuristic through the generator.
#![allow(clippy::useless_format)]
#![allow(clippy::nonminimal_bool)]

include!(concat!(env!("OUT_DIR"), "/spec_tests.rs"));
