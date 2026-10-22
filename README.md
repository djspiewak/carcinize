# carcinize

A small Rust application intended to talk to the GitHub API.

This initial scaffold has no API operations or authentication yet and makes
no network requests. Dependencies will be added with the first concrete API
workflow; the agent guidance calls for `reqwest` for outbound HTTP.

## Development

Use stable Rust with Cargo, rustfmt, and Clippy. The initial setup was verified
with Rust 1.99.1 and uses edition 2024; a minimum supported Rust version has
not been established.

```sh
cargo run --locked
make verify
```

`make fmt` formats the source. `make verify` checks formatting, compiles all
targets, runs Clippy with warnings denied, and runs tests. There are no tests
yet because no application behavior has been implemented. Keep `Cargo.lock`
committed for reproducible dependency resolution.

Read [AGENTS.md](AGENTS.md) and the imported
[Rust engineering guidance](docs/agents/rust.md) before implementing features.
