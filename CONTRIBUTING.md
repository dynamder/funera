# Contributing to funera

Thanks for your interest in contributing! funera is an LLM agent framework for
Rust. We welcome bug reports, feature requests, documentation improvements, and
code contributions of all sizes.

## Getting started

The repository pins its Rust toolchain via [`rust-toolchain.toml`](rust-toolchain.toml)
(stable + `rustfmt` + `clippy`). Install Rust with [rustup](https://rustup.rs),
then:

```sh
git clone https://github.com/dynamder/funera
cd funera
cargo build --all-features
cargo test --workspace --all-features
```

## Minimum supported Rust version (MSRV)

funera's MSRV is **Rust 1.88** (it uses `let`-chains, stabilized in 1.88). The
`rust-version` field in each `Cargo.toml` declares this, and CI verifies it with
a pinned toolchain.

Raising the MSRV is a **minor** version change. If a change needs a newer
compiler, call it out in the pull request and CHANGELOG.

> Note: the `sandbox` feature on Linux/macOS pulls in `nono`, whose own MSRV is
> higher; the declared MSRV covers the default build.

## Workspace layout

| Crate | Role |
|-------|------|
| `funera` | Top-level crate re-exporting the orchestration and core APIs |
| `funera-core` | Core engine: ReAct loop, providers, tools, skills, middleware, security, and the plugin system |
| `funera-orchestrate` | High-level builder API (`Agent`, `AgentRuntime`) |
| `funera-builtin-tools` | Bundled Read/Write/Edit/Shell tools |

## Development workflow

Before opening a pull request, run the full local gate:

```sh
cargo fmt --check --all                                # formatting
cargo clippy --all-features -- -D warnings             # lints (warnings denied)
cargo test --workspace --all-features                  # unit + integration tests
cargo test --doc --all-features --workspace            # doctests
cargo doc --no-deps --all-features                     # docs build
```

### Feature flags

funera is heavily feature-gated so consumers pay only for what they use. See the
[README](README.md#features) for the full table. When testing a change, exercise
the relevant combinations — CI covers, for `funera-core`, `default`, `security`,
and `security,middleware`; and for `funera-orchestrate`, everything from minimal
to `all-features`, on Linux, Windows, and macOS.

### Mutation testing

We use [`cargo-mutants`](https://github.com/sourcefrog/cargo-mutants) to check
test quality on new logic:

```sh
cargo install cargo-mutants
cargo mutants -p funera-core --file 'funera_core/src/plugin.rs' --all-features
```

Aim for zero `missed` mutants on the code you touch.

## Code style

- Follow `rustfmt` — CI runs `cargo fmt --check --all`.
- Keep `clippy` clean — CI runs `cargo clippy --all-features -- -D warnings`.
- Write unit tests alongside new logic and doctests for public API where useful.
- Document public items with `///` so `cargo doc` stays complete.

## Submitting changes

1. Fork the repository and create a feature branch off `main`.
2. Make your changes, with tests.
3. Run the full local gate above.
4. Open a pull request against `main`, linking any related issue.
5. Use the pull-request template and keep the commit history clean.

### Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/) prefixes:
`feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`, `perf:`. Keep the
summary line under ~72 characters.

## Release process

Releases are automated by [`cd.yml`](.github/workflows/cd.yml) (manual workflow
dispatch): it bumps the version with `cargo-release`, re-runs the full gate,
publishes the crates to crates.io in dependency order, and creates a GitHub
release. Maintainers trigger it from the Actions tab.

## Community

- Be respectful and inclusive — see the [Code of Conduct](CODE_OF_CONDUCT.md).
- Report security issues privately — see [SECURITY.md](SECURITY.md).
