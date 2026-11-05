# Lantern — rules for every session

Lantern is built on [Krifmaz/kit](https://github.com/Krifmaz/kit) (v0.2.1): its server crates, kit-leptos
and the kit's default theme. Read the kit's `CHANGELOG.md` before bumping the kit tag.

- **No shell for user input.** Never pass anything a person typed, uploaded or named to a shell. Run programs
  with an argument list (`std::process::Command::new(..).arg(..)`), never `sh -c`.
- **Full doc comments.** Every module, type, function and field says what it is for, in plain words.
- **Plain wording.** Say what happened and what to do next, in short sentences a person who is not a
  programmer understands. No codes, no jargon, no blame.
- **Copyable errors.** Every error is shown in full and can be copied (the kit's toasts and details dialog do
  this); errors answer `{"detail": "<plain sentence>"}` with the right HTTP status (kit-server-core's `err`).
- **The kit's look.** Use the kit's theme (the default look on `:root`; `data-theme` `dark`, `oled` or `light`
  for the others) and components; no colours, fonts or sizes of your own outside the generated tokens.
- **Small validated changes.** One change at a time, each with its test. `cargo fmt`, `cargo clippy --workspace
  --all-targets -- -D warnings` and `cargo test --workspace` pass before every commit. Every change gets a line in `CHANGELOG.md`.
- **Desktop.** `cd desktop && cargo test` draws every page headless; every server path lives in
  `desktop/src/paths.rs`; release file names from `scripts/desktop-package.sh` must match `desktop/src/release.rs`
  (`file_for`).
- **Android.** `android/` is a Gradle build of its own on kit-compose (same kit tag); ktlint and `./gradlew test`
  pass; the kit's `kit-android.yml` builds and tests it in CI.
- **Safety.** The container is distroless and runs as a non-root user with a read-only root filesystem; write
  only to the data folder (`APP_DATA_DIR`, `/data` in the image).
- **Releases.** Every merge to main is a normal release `vMAJOR.MINOR.PATCH` (never a pre-release). PATCH by
  default; add `docs/releases/<version>.md` in the PR for MINOR or MAJOR (it is also the release notes and
  What's new).

## Shared app block

Build on Krifmaz/kit (pin the latest kit tag; read its docs/DESIGN.md and CLAUDE.md first). Use the kit for the look (Encore theme), What's new and changelog, self-updating web app, notice banners, smart toasts with Copy, confirm dialogs, bug reports and crash logs, diagnostics, settings from a schema, offline mode and the shared release workflows. Don't rebuild what the kit has; if something is missing or generic, add it to the kit instead of the app.
Work hands-off: make your own calls (log non-obvious ones in docs/DECISIONS.md), one PR per change, fix CI until green, squash-merge yourself, delete the branch. Every merge ships a normal vX.Y.Z release (no pre-releases). Never skip tests or force-push main. Only stop to ask about deleting user data, a needed secret or paid service. One short summary per finished goal.
