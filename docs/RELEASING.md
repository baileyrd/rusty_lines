# Release checklist

Steps to cut a `rusty_lines` release. Work through them in order; each
release should be a PR like any other change.

## 1. Pre-flight

- [ ] `main` is green in CI — all six jobs (Linux/macOS tests, Windows
      check, `Feature (rusty-libc)`, clippy+rustfmt+doc, MSRV).
- [ ] Local checkout is synced to `main` with a clean tree.
- [ ] Full local sweep passes:

  ```sh
  cargo test
  cargo test --features rusty-libc
  cargo clippy --all-targets -- -D warnings
  cargo clippy --all-targets --features rusty-libc -- -D warnings
  cargo fmt --check
  RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
  cargo check --all-targets --target x86_64-pc-windows-gnu
  ```

- [ ] MSRV is consistent in all three places: `rust-version` in
      `Cargo.toml`, the CI `msrv` job's toolchain, and the CHANGELOG
      entry that last changed it.
- [ ] The feature matrix is identical in `README.md` and
      `docs/survey.md` (they are deliberately duplicated).

## 2. crates.io blockers

- [ ] **The `rusty_libc` git dependency must go.** crates.io rejects
      git dependencies, even optional ones. Either:
      - publish `rusty_libc` to crates.io first and switch to
        `rusty_libc = { version = "...", optional = true }`, or
      - drop the optional dependency (and the `rusty-libc` feature +
        CI job) for this release and restore it afterwards.
- [ ] `cargo package --list` — check nothing unexpected ships and
      nothing needed is missing.
- [ ] `cargo publish --dry-run` passes.

## 3. Version bump (as a PR)

- [ ] Bump `version` in `Cargo.toml`. Pre-1.0 semver: breaking changes
      bump the minor version. (`save_history` taking `&mut self` since
      0.1.0 is an example of a breaking change.)
- [ ] CHANGELOG: retitle `## Unreleased` to `## X.Y.Z` with the date;
      add a fresh empty `## Unreleased` above it.
- [ ] Open the release PR, wait for CI, merge, sync.

## 4. Tag and publish

- [ ] Tag the merge commit and push the tag:

  ```sh
  git tag -a vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z
  ```

- [ ] `cargo publish`
- [ ] Create a GitHub release for the tag, body = that version's
      CHANGELOG section.

## 5. Post-release

- [ ] Verify the docs.rs build succeeded and the crate page renders
      (README, badges, feature docs).
- [ ] Point [rush](https://github.com/baileyrd/rush) at the released
      version instead of a path/git dependency, and run its downstream
      pty harness against it.
- [ ] If the `rusty_libc` dependency was dropped in step 2, restore it
      on `main`.
