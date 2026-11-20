# Contributing to obojeno

Thanks for helping. Bug reports, ideas and pull requests are all welcome. For
anything larger than a fix, open an issue first so we can agree on the
approach before you put time in.

## Setup

You need macOS 26 or later and Xcode (or the Command Line Tools) with Swift
6.2, for the menu bar app. For Rust, either use Nix:

```sh
nix develop      # rustc, cargo, clippy, rustfmt, rust-analyzer, cargo-insta,
                 # cargo-deny, shellcheck, actionlint
```

or install Rust 1.97 or later with rustup.

```sh
cargo test --workspace                       # everything in Rust
./scripts/build-app.sh                       # target/Obojeno.app
target/Obojeno.app/Contents/Helpers/obojeno -v daemon   # a daemon in the terminal
```

The app starts its own daemon when none runs. To try a change to the daemon,
quit the app (or `pkill -f "obojeno.*daemon"`) and start the new one.

## Where things are

| Path | What |
|---|---|
| `crates/obojeno-core` | Platform-free: palettes, templates, config, gallery model and the socket protocol (`ipc.rs`) |
| `crates/obojeno-core/templates` | The built-in targets' templates |
| `crates/obojeno` | The macOS binary: CLI, daemon, wallpaper transitions, lid animation, finish, app reloads |
| `app` | The SwiftUI menu bar app; a client of the daemon's socket |
| `nix`, `flake.nix` | Nix package and home-manager module |
| `scripts` | Building the app, the Homebrew cask, version checks |

The daemon owns all state. The CLI and the app send it one JSON line per
request over `~/.local/state/obojeno/obojeno.sock` (see `ipc.rs`).

## Before you open a pull request

CI runs these; run them first:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
swift format lint --strict -r app/Sources app/Tests
swift test --package-path app
shellcheck scripts/*.sh
actionlint                                   # the GitHub workflows
cargo deny check                             # licenses and advisories
```

Template changes update snapshots: review them with `cargo insta review`.

For anything you can see, try it in the app as well: transitions, the lid
animation, the gallery and settings. Add a screenshot or a short recording to
the pull request.

## Conventions

- Clippy runs with `pedantic`. Where a lint does not fit, use
  `#[expect(clippy::...)]` with a comment saying why, not `#[allow]`.
- Every `unsafe` block gets a `// SAFETY:` comment.
- Comments explain why, not what.
- Keep the daemon small: it runs all day. Decode images through `imageio`, which
  uses helper processes in the daemon. Give Core Animation `IOSurface`s rather
  than images it has to draw. Core Graphics keeps a copy of everything it
  draws.
- Test what you change; `crates/obojeno-core/tests` checks palettes against
  Google's reference implementation.

## Adding an app to theme

A built-in target is a template in `crates/obojeno-core/templates`, an entry
in `crates/obojeno-core/src/targets.rs`, and, if the app can reload without a
restart, a reloader in `crates/obojeno/src/reload.rs`. `detect.rs` and
`setup.rs` cover finding the app and the one-time setup it needs. Anything
else can already be themed as a custom target from `config.toml`.

## Releases

Maintainers release by pushing a `v<version>` tag that matches `Cargo.toml`
(`scripts/version.sh`); `.github/workflows/release.yml` does the rest,
including the signed update feed (`appcast.xml`) that installed copies update
from. It needs the `SPARKLE_PRIVATE_KEY` secret, whose public half is in
`scripts/build-app.sh`; a different key would strand every installed copy.

## License

By contributing, you agree that your contributions are licensed under the MIT
license, like the rest of obojeno.
