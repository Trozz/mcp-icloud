# Contributing

## Build and test

You need Rust 1.99.1 (the workspace pins it).

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check
bash scripts/test-next-version.sh
```

Live smoke tests against your own account are ignored by default:

```bash
ICLOUD_EMAIL=... ICLOUD_APP_PASSWORD=... cargo test -p mcp-icloud-mail --test live_icloud -- --ignored --nocapture
ICLOUD_EMAIL=... ICLOUD_APP_PASSWORD=... cargo test -p mcp-icloud-calendar --test live_icloud -- --ignored --nocapture --test-threads=1
ICLOUD_EMAIL=... ICLOUD_APP_PASSWORD=... cargo test -p mcp-icloud-contacts --test live_icloud -- --ignored --nocapture --test-threads=1
```

The calendar round trip creates one clearly named event in December 2030 and always deletes it. The contacts round trips create contacts named `ZZ MCP Probe` and a group `ZZ MCP Probe Group` with made-up data (the merge test merges two such contacts) and always delete them. The invitation test runs only when `ICLOUD_TEST_ATTENDEE` names an address you control.

## This documentation

The site is built with [mdBook](https://rust-lang.github.io/mdBook/) from `docs/`. To preview it:

```bash
cargo install --locked mdbook@0.5.4 mdbook-mermaid@0.17.1 lychee@0.24.2
bash scripts/build-docs.sh
mdbook serve docs --open
```

`scripts/build-docs.sh` builds the book and checks every internal link and anchor. A test in the `mcp-icloud` crate checks that every environment variable the server reads appears in the [configuration reference](configuration.md) and every registered tool appears on its service page, so the docs cannot fall behind the code.

## Adding a service

Each service is a library crate that implements the `Service` trait from `mcp_server_core::service`:

1. Create `crates/mcp-icloud-<name>` as a library. Read its own variables (plus `ICLOUD_EMAIL` and `ICLOUD_APP_PASSWORD`) in `pub fn from_env(env: &mut ServiceEnv) -> Option<Arc<dyn Service>>`, reporting problems through `env.errors`. Never read the server variables.
2. Build the MCP endpoint in the trait's `router()` with `mcp_server_core::service::mcp_service`, and return the same MCP server from `handler()` so the combined endpoint can offer it. Service names are lowercase path segments without `_` (the combined endpoint uses `_` to separate the service from the tool name), and `mcp` is reserved.
3. Add the crate to `crates/mcp-icloud/Cargo.toml` and one line to `SERVICES` in `crates/mcp-icloud/src/lib.rs`.
4. Document its tools on a new page under `docs/src/services/` and its variables in `docs/src/configuration.md`.

Routing, OAuth metadata, token audiences, `ICLOUD_SERVICES`, read-only mode, audit, the combined endpoint and the release pipeline pick the new service up without further changes.

## Pull requests

- Branch from `develop` as `feature/<name>`, `bugfix/<name>` or `hotfix/<name>`, and open the pull request against `develop`.
- Use conventional commit messages (`feat:`, `fix:`, `docs:`, `test:`, `chore:`, `refactor:`).
- Test data must be synthetic: never copy real mail, events or contacts into fixtures.
- CI runs fmt, clippy, tests, cargo-deny, a Docker build, the script tests and the documentation build.
