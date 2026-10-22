# Project guidance

Read `docs/agents/rust.md` before Rust or dependency changes. This is the
Rust engineering guidance from tperrett/rust-scaffold; provenance is in
`docs/agents/README.md`. User instructions take precedence.

- Prioritize composability and testability. Keep domain logic stateless and
  synchronous; pass dependencies explicitly and isolate I/O at boundaries.
- Keep `src/main.rs` limited to application wiring as functionality grows.
  Introduce focused modules when needed, without speculative abstractions.
- Use `reqwest` with Rustls and explicit connect, read, and request timeouts
  when adding the GitHub HTTP adapter. Keep dependency features narrow.
- Keep credentials out of source, diagnostics, fixtures, and committed files.
- Test GitHub interactions through a local boundary fake; ordinary tests must
  not require network access or real credentials.
- Use `make verify` for formatting, compilation, Clippy, and tests. Add
  documentation tests to that gate when a library target is introduced.
- Preserve unrelated changes and do not commit unless asked.
- Use the user's home review skills and `reviewer`/`review_fixer` custom agents
  for delegated review work, following the user's routing instructions.

The project currently contains only a binary scaffold. No GitHub API commands,
authentication mechanism, server, or deployment configuration exist yet.
