# Project guidance

Read `docs/agents/rust.md` before Rust or dependency changes.

- Prioritize composability and testability. Keep domain logic stateless and
  synchronous; pass dependencies explicitly and isolate I/O at boundaries.
- Keep `src/main.rs` limited to application wiring.
  Introduce focused modules when needed, without speculative abstractions.
- Keep dependency features narrow and configure explicit timeouts for network I/O.
- Keep credentials out of source, diagnostics, fixtures, and committed files.
- Test external interactions through local boundary fakes; ordinary tests
  must not require network access or real credentials.
- Use `make verify` for formatting, compilation, Clippy, unit/boundary tests,
  and documentation tests.
- Preserve unrelated changes and do not commit unless asked.
- Use the user's home review skills and `reviewer`/`review_fixer` custom agents
  for delegated review work, following the user's routing instructions.
