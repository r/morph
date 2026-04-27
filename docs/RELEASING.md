# Releasing Morph

This document is the runbook for cutting a Morph release and publishing the binaries to the Homebrew tap. It is written for the maintainer who is about to push a tag, not for end users.

End users install with `brew install r/morph/morph` (or `cargo install --locked --path morph-cli` from a checkout). They don't need to read this file.

---

## Release artifacts

Each release produces, for every supported target, a `morph-<target>.tar.gz` and a `morph-<target>.tar.gz.sha256` containing both binaries:

- `morph` — the CLI.
- `morph-mcp` — the MCP server.

Supported targets (built by `.github/workflows/release-homebrew.yml`):

| Target | Built on | Used by |
|---|---|---|
| `aarch64-apple-darwin` | `macos-14` | Apple Silicon Macs |
| `x86_64-apple-darwin` | `macos-13` | Intel Macs |
| `aarch64-unknown-linux-gnu` | `ubuntu-latest` (cross) | ARM Linux (Raspberry Pi 5, Graviton) |
| `x86_64-unknown-linux-gnu` | `ubuntu-latest` | Most Linux servers and laptops |

Binaries are stripped and tarballs are SHA-256 checksummed. The Homebrew formula pins the per-target SHA so an in-flight tarball substitution would fail formula installation.

---

## Versioning

Morph uses a single workspace-level version. It lives in **one** place:

```toml
# /Cargo.toml (workspace root)
[workspace.package]
version = "X.Y.Z"
```

All crates inherit it via `version.workspace = true`. The build date is embedded at compile time via the `MORPH_BUILD_DATE` env var (set by `build.rs`).

While Morph is pre-1.0:

- **Minor** (`0.X.0`) — new commands, new MCP tools, breaking CLI changes.
- **Patch** (`0.0.X`) — bug fixes and small improvements.

The release workflow refuses to publish a tag whose name doesn't match the workspace version. So `v0.16.0` only works if `Cargo.toml` says `0.16.0`.

---

## Cutting a stable release

1. **Bump the version** in the workspace `Cargo.toml`.
2. **Run the workspace test suite locally** and confirm it is green:

   ```bash
   cargo test --workspace --locked
   ```

3. **Update `morph-cli/tests/specs/version.yaml`** so the spec test asserts the new version string.
4. **Record evaluation metrics** for the release commit:

   ```bash
   morph eval record metrics.json
   ```

   See `.cursor/rules/behavioral-commits.mdc` for the metrics shape.
5. **Commit and push to `main`**.
6. **Tag and push the tag**:

   ```bash
   git tag v0.16.0
   git push origin v0.16.0
   ```

   The push triggers `.github/workflows/release-homebrew.yml`, which:

   - Runs the full test suite (`test` job — release blocks on this).
   - Builds the four target tarballs in parallel (`build-artifacts`).
   - Smoke-tests each native binary by parsing `morph version --json`.
   - Publishes a GitHub release at `v0.16.0` with all eight files (`publish`).
   - Updates the Homebrew formula in the tap repo with the new version, URLs, and per-target SHA-256s (`update-tap`).

7. **Verify the release** in three places:

   - GitHub Releases page: tarballs + checksums attached, release notes auto-populated.
   - Tap repo: `Formula/morph.rb` updated with the new version and SHAs.
   - Locally: `brew update && brew upgrade morph && morph version --json` should report the new version.

---

## Nightly / commit builds

Pushes to `main` trigger the same workflow, but the `update-tap` job is gated by `is_tag_release`. So commit pushes:

- **Do** run tests, build all targets, publish a `commit-<sha>` GitHub release.
- **Do not** touch the Homebrew tap.

This is intentional: commit builds let us catch packaging regressions immediately, but the tap only follows stable tags. Power users can pin a specific commit by downloading the corresponding `commit-<sha>` artifacts directly from the Releases page.

---

## Smoke test contract: `morph version --json`

The release workflow validates each freshly built `morph` binary by running:

```bash
morph version --json
```

and asserting the JSON has these keys:

- `name` — always `"morph"`.
- `version` — must equal the workspace version embedded at build time.
- `build_date` — RFC 3339 UTC timestamp baked in by `build.rs`.
- `protocol_version` — the SSH wire protocol version (`MORPH_PROTOCOL_VERSION`).
- `supported_repo_versions` — array of repo schema versions this binary can read (currently `["0.0", "0.2", "0.3", "0.4", "0.5"]`).

This shape is also tested in `morph-cli/src/main.rs::tests::version_json_has_stable_field_set` and is **additive only** — adding fields is fine, removing or renaming a field is a breaking change for any pipeline that consumes the JSON.

The Homebrew formula's `test do` block also exercises `morph version --json`, so a corrupt or incompatibly built tarball will fail `brew test morph` on the user's machine.

---

## Required GitHub configuration

The workflow needs two pieces of repository configuration to update the tap. Without them, the `update-tap` job will fail loudly (the build and publish jobs still succeed):

| Name | Type | Description |
|---|---|---|
| `HOMEBREW_TAP_TOKEN` | Secret | A fine-grained PAT with **contents: write** on the tap repo. Used to push the formula update. |
| `HOMEBREW_TAP_REPO` | Variable | The tap repo, in `owner/homebrew-name` form (e.g. `r/homebrew-morph`). |

To configure them:

1. **Create the tap repo** if it doesn't exist. Convention: `homebrew-morph` under the same owner as the main repo. Initialize it with an empty `Formula/` directory and a short README.
2. **Create the PAT** at github.com/settings/tokens?type=beta:
   - Repository access: only the tap repo.
   - Permissions: Contents → Read and write.
   - Expiration: as long as your security policy allows; the workflow will fail with a clear error when the token expires.
3. **Add the secret and variable** to the main repo:
   - Settings → Secrets and variables → Actions → New repository secret → `HOMEBREW_TAP_TOKEN` = the PAT.
   - Settings → Secrets and variables → Actions → Variables → New repository variable → `HOMEBREW_TAP_REPO` = `owner/homebrew-morph`.

---

## What is *not* yet automated

These are deliberate gaps that need a maintainer decision before code can fill them in:

- **Sigstore / cosign signing** of release tarballs. The release pipeline does not currently sign artifacts. Adding `cosign sign-blob` to the publish step is straightforward once a signing identity is chosen (keyless OIDC vs. a long-lived key).
- **Notarization on macOS**. Binaries are unsigned; users will see a Gatekeeper warning on first run unless they `xattr -d com.apple.quarantine`. Notarizing requires an Apple Developer ID and adds ~5 minutes per build. Not blocking for now; revisit before 1.0.
- **Reproducible builds**. Builds use the default toolchain channel; we don't yet pin a specific stable release in `rust-toolchain.toml`. Reproducibility-curious users should pin a toolchain in their fork.
- **Linux packaging beyond a tarball**. No `.deb`, no `.rpm`, no `apt` repo. Homebrew on Linux works; native Linux distro packaging is a future line of work.

---

## Rolling back a release

If a tagged release is shipped and turns out to be broken, the safest rollback is to ship a new tag with a fix rather than rewriting history:

1. Fix the bug, bump the patch version (`0.16.0` → `0.16.1`).
2. Push the new tag. The Homebrew formula is overwritten, so users get the fix on the next `brew upgrade`.

Only rewrite a tag (delete + repush) if the broken release is **less than an hour old** and you're certain no one has installed it yet — Homebrew bottles are content-addressed by SHA, so a republished tag with the same name but a different binary will hash-mismatch on user machines.

---

## See also

- `.github/workflows/release-homebrew.yml` — the workflow itself.
- `.cursor/rules/version-bump.mdc` — when to bump major/minor/patch.
- `.cursor/rules/behavioral-commits.mdc` — recording metrics with each commit.
- [INSTALLATION.md](INSTALLATION.md) — what users see on the other end.
