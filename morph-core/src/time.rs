//! Timestamps in canonical Morph form.
//!
//! Every Morph object that carries a `timestamp`, `forgotten_at`,
//! `recorded_at`, or similar field uses **RFC-3339 in UTC** with
//! the form produced by [`chrono::DateTime::to_rfc3339`]. Funnel
//! every "now" through [`now_rfc3339_utc`] so the on-wire string
//! shape is set in exactly one place — handy when we eventually
//! want to swap precision, inject a fake clock for tests, or
//! enforce a canonical sub-second policy.

/// Current wall-clock time in UTC, formatted as RFC-3339.
///
/// Equivalent to `chrono::Utc::now().to_rfc3339()`. The shape is
/// `2026-05-01T22:34:09.123456789+00:00` — preserves the existing
/// on-disk timestamp format used across `Commit`, `Run`, `Trace`,
/// `Annotation`, `Tombstone`, and friends.
pub fn now_rfc3339_utc() -> String {
    chrono::Utc::now().to_rfc3339()
}
