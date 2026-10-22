# Rust Engineering Rules

These rules apply to Rust code created or modified in this repository. They
supplement the repository's `AGENTS.md`; project-local instructions take
precedence where they are more specific.

The goal is correct, explicit, testable, maintainable, and unsurprising code.
Prefer simple, composable designs over clever abstractions. When trade-offs are
necessary, prefer, in order: correctness, clarity, testability,
maintainability, performance, then concision.

## 1. Core Design Principles

- Use stable, idiomatic Rust and the repository's current edition. Prefer
  explicit, readable code over type gymnastics.
- Make invalid states difficult to represent. Prefer enums over related
  booleans, newtypes over interchangeable primitives, validated constructors
  over unrestricted fields, and explicit state transitions over independent
  mutation of related fields.
- Use meaningful names and Rust conventions: `snake_case` for modules,
  variables, and functions; `PascalCase` for types, traits, structs, and enum
  variants.
- A caller should be able to tell what a function reads, modifies, can fail
  with, whether it performs I/O or blocks, and whether it starts background
  work. Avoid hidden control flow, global state, and implicit side effects.
- Prefer immutable bindings. Keep necessary mutation local, brief, and
  invisible outside the function. Do not use interior mutability to evade an
  unclear ownership design.
- Keep domain logic synchronous and pure wherever possible. Isolate filesystem,
  network, database, clock, randomness, and process interaction at adapters or
  orchestration boundaries.
- Pass dependencies explicitly through function arguments, constructors, small
  context structs, or narrowly scoped boundary traits. Inject nondeterministic
  inputs such as time rather than reading them in domain logic.
- Do not introduce speculative frameworks, traits, configuration, or extension
  points. Prefer the smallest design that cleanly meets the current need.

The preferred flow is:

```text
input adapters -> parsing and validation -> pure domain logic
    -> result or command model -> output adapters
```

Do not combine parsing, I/O, domain decisions, and presentation in one
function.

## 2. Types, Ownership, and Errors

### Types and ownership

- Parse external strings once at the boundary, then use typed values internally.
  Use structs, enums, and newtypes instead of strings and loosely shaped maps.
- Borrow when ownership is unnecessary: use `&str` instead of `&String`,
  `&[T]` instead of `&Vec<T>`, and iterators where they preserve clarity.
- Do not clone merely to satisfy the borrow checker. Clone when ownership must
  truly be duplicated, the data is cheap, or it substantially simplifies a
  proportionate design. Make non-obvious ownership reasons visible in the code.
- Use a typed input/options struct when related parameters grow beyond about
  five. Do not use unclear boolean arguments.
- Keep fields private by default. Public fields are for deliberate,
  unrestricted data-transfer types; use constructors and accessors where an
  invariant must hold.
- Use exhaustive enums for finite state. Avoid wildcard match arms when a new
  variant should force a review of the handling logic.

### Errors

- Use `Result<T, E>` for recoverable or operational failures. `Option<T>` is
  only for legitimate absence; use `Result<Option<T>, E>` when both absence and
  failure are possible. Never use sentinel values when a reason matters.
- Do not use `unwrap`, `expect`, `panic!`, `unreachable!`, panic-prone indexing,
  or unchecked arithmetic in production flow. A production `expect` is allowed
  only for a statically or structurally guaranteed invariant, with a message
  explaining why failure is impossible. Tests may use `expect` to state setup
  invariants.
- Use `thiserror`-style typed enums at reusable domain, API, and persistence
  boundaries where callers need a stable error or status mapping. Use `anyhow`
  at CLI and workflow entry points where rich context matters more than
  exhaustive matching. Do not expose `anyhow::Error` from reusable library APIs
  unless that API is deliberately application-specific.
- Preserve error sources and add context at every fallible I/O or external
  boundary. An error should name the operation, resource, method/path or file,
  and useful non-secret diagnostic detail. Do not flatten sources into opaque
  strings.
- Log an error once near the boundary that handles it; lower layers should
  return enriched errors rather than logging every propagation step.
- For HTTP APIs, map errors deliberately: validation to 400/422, absence to
  404, conflicts to 409, authentication/authorization to 401/403, upstream
  failures to 502/503/504, and unexpected internal failures to 500.

## 3. Validation, Determinism, and Durable State

- Separate parsing (can the input be structurally interpreted?) from validation
  (is the interpreted value permitted by the domain?) where practical.
- Validate untrusted data at admission boundaries, before storing it or calling
  a solver or upstream. Reject malformed JSON/YAML, unknown configuration
  fields unless extension is intended, missing required semantics, blank IDs,
  invalid encodings, oversized values, non-finite floats, invalid numeric
  ranges, and unknown resource factors.
- Use `serde` derives for boundary types. Prefer `deny_unknown_fields` for
  configuration and request shapes unless backwards-compatible extension is a
  stated requirement; apply `rename_all` consistently for stable wire formats.
- Return complete, source-located diagnostics for documents and configuration
  when independent validation errors can be collected safely. Stop when an
  error makes later validation unreliable.
- Generated files, manifests, diagnostics, snapshots, solver inputs, and tests
  must be deterministic unless nondeterminism is an explicit requirement. Do
  not depend on `HashMap` or `HashSet` iteration order; sort before producing
  observable output or use ordered collections when the order is significant.
- Select collections intentionally: `Vec` for ordered sequences, `HashMap` for
  unordered lookup, `BTreeMap` for deterministic key ordering, `HashSet` for
  unordered uniqueness, `BTreeSet` for ordered uniqueness, and `VecDeque` for
  queues.
- Prefer fail-closed behavior for correctness, security, authorization,
  resource decisions, and durable state. A plausible default is dangerous when
  it can silently corrupt an allocation, metric, permission, or record.
- Avoid read-modify-write races in durable state. Use revision checks,
  compare-and-swap, per-resource locking, or retries with fresh reads whenever
  two writers can affect the same record.
- Make externally visible side effects truly idempotent. Persist an in-progress
  or completed record and request fingerprint before the external write when
  possible; reject reuse of an idempotency key with different inputs; treat
  duplicate starts and retries as expected behavior.
- Treat writes to external systems as high risk. Require a preview, fingerprint
  or idempotency pattern where appropriate, and retain an audit trail that can
  explain what happened without exposing secrets.

## 4. Async and Concurrency

- Use async only for operations that wait on asynchronous I/O, databases,
  timers, queues, or async subprocesses. Do not make pure computation async
  simply because the caller is async.
- Async functions should orchestrate I/O and delegate computation to synchronous
  pure functions. Never perform blocking filesystem traversal, CPU-heavy work,
  blocking database calls, `std::thread::sleep`, blocking subprocess waits, or
  large compression work directly on the async runtime. Use an async-native API,
  `spawn_blocking`, or a bounded dedicated worker pool.
- Treat each `.await` as an intentional scheduling point. Do not hold a lock,
  borrowed resource, or partially updated state across `.await` unless the
  invariant requires it and the reason is documented.
- Do not spawn unbounded work for arbitrary-size input. Bound concurrency with
  semaphores, bounded channels, worker pools, buffered streams, queues, or
  explicit batches. Name the limit or make it operational configuration.
- Every spawned task needs a clear owner, error-observation path, cancellation
  behavior, and shutdown policy. Do not discard `JoinHandle`s unless the task
  is intentionally detached and that decision is documented.
- Treat cancellation and timeouts as first-class behavior. Long-running work
  should have cancellation checkpoints where practical and distinct operational
  states for timeout, cancellation, panic, retry exhaustion, and upstream
  failure when callers need to react differently.
- Use `tokio::select!` to coordinate cancellation, timeouts, and competing
  work. Keep branches small enough that cancellation behavior is clear.
- Use bounded `tokio::sync::mpsc` channels for queued work, `broadcast` for
  fan-out, and `oneshot` for single handoffs. Handle full and closed channels
  deliberately; never silently drop work.
- Use `tokio::sync::Mutex`/`RwLock` only for shared state that crosses tasks.
  Keep critical sections small and avoid deadlock-prone designs under retry or
  cancellation. Use `tokio::time` for timers and intervals.
- Introduce async traits only at meaningful architectural boundaries. Prefer
  concrete types when polymorphism is not required, and do not create traits
  solely to mock every dependency.

## 5. Application Library and Boundary Conventions

- Use `tokio` for services, workers, and async CLI paths, enabling only needed
  features. Use `axum` for new HTTP routes and composition; follow existing
  local patterns rather than changing framework in unrelated work.
- Keep route handlers thin: extract/authenticate inputs, call domain or service
  functions, and map typed errors to responses. Test routers in process with
  `tower` where socket/TLS/connect-info behavior is not under test.
- Use `reqwest` for outbound HTTP. Build clients through a shared helper that
  supplies connect, read, and total-request timeouts. Prefer Rustls features
  unless a platform constraint requires another TLS backend.
- Use `clap` derive and structured subcommands for CLI parsing. Successful CLI
  output is JSON on stdout; human guidance, progress, and interactive prompts
  belong on stderr.
- Use `serde`, `serde_json`, and `serde_yaml` for structured data. Never parse
  JSON or YAML with ad hoc string manipulation. YAML remains the default for
  internal service and deployment configuration unless an established interface
  requires another format.
- Use `tracing` spans and structured fields at external requests, durable
  writes, long-running jobs, parsing/compilation phases, task execution, and
  retry loops. Propagate trace context where available; use the existing
  OpenTelemetry and Prometheus integrations for production observability.
- Use Askama for template-driven operational UI and reuse layouts or helpers.
  Use ULIDs for locally generated durable IDs when sortable chronology helps,
  and SHA-2 for non-secret fingerprints and integrity checks.
- Use proven domain crates for solvers, workflow SDKs, databases, TLS/JWT, and
  parsers. Do not hand-roll protocols, cryptography, scheduling, optimization,
  or unsafe data structures when a maintained library fits.
- Keep dependency features intentional. Prefer `default-features = false` for
  heavy infrastructure crates when the needed feature set is known.

## 6. Security and Resource Limits

- Keep security defaults safe. Development bypasses must be explicit, narrowly
  scoped, named as unsafe where appropriate, and covered by tests proving
  production-like paths remain protected.
- Never log passwords, API keys, tokens, private keys, session cookies,
  unredacted credentials, or sensitive configuration. Apply the repository's
  data-sensitivity rules to user-provided content.
- Bound untrusted or potentially large inputs: file/allocation/output size,
  nesting depth, collection length, concurrency, retries, and total time.
- For filesystem paths, define whether absolute paths are allowed, reject
  traversal outside configured roots, do not rely on string-prefix containment,
  and handle symlinks according to an explicit policy.
- Construct subprocesses with direct argument APIs, never shell command strings
  built from untrusted input.
- Avoid `unsafe` in product code. If it is unavoidable, isolate the smallest
  possible block behind a safe, documented API. Every block needs a `SAFETY:`
  comment explaining its soundness invariants and needs boundary/invariant tests;
  run Miri and sanitizers where the code and platform support them.

## 7. Modules, Files, and Public APIs

- Keep `main.rs` minimal: wire modules and delegate to CLI/server entry points.
  Organize by cohesive domain concepts and boundaries, not generic `utils`,
  `helpers`, or `common` dumping grounds.
- Separate domain models, parsing, validation, orchestration, persistence,
  external clients, presentation/serialization, CLI handling, and configuration
  where that division clarifies ownership.
- Keep route modules focused on routing and handler glue. Durable state,
  validation, comparisons, lifecycle policy, and external writeback belong in
  submodules named for the concept they own.
- Default visibility is private; use `pub(crate)` for intentional crate-internal
  interfaces and `pub` only for a deliberate external contract.
- A source file approaching 500 lines is a design signal. Inspect it for
  separate responsibilities and split it before it becomes a multi-purpose
  dumping ground. Generated files and declarative tables may exceed this when a
  split would reduce clarity; existing large files are not a pattern to copy.
- Keep module depth modest: nesting should communicate architecture, not merely
  hide file size.
- Public types and functions must document purpose, important invariants,
  notable side effects, errors, panics, and safety requirements where relevant.
  Comments explain *why*, not obvious mechanics.

## 8. Tests and Test Data

Testing emphasizes observable behavior, invariants, and failure modes rather
than implementation details. Keep tests deterministic, fast, and named for the
behavior and expected outcome.

- Cover pure behavior with unit tests; subsystem boundaries with integration
  tests; essential user workflows with a small end-to-end suite; and every bug
  fix with a regression test that failed before the fix.
- Unit tests should include normal behavior, boundaries, empty/malformed input,
  state transitions, error variants, and domain invariants. Use Arrange–Act–
  Assert where it makes the behavior clearer.
- Use property tests (for example `proptest`) for important solver/model,
  normalization, parser, serialization, ordering, pagination, state-machine,
  identifier, and deterministic-allocation invariants. Good properties include
  round trips, idempotence, conservation, ordering, no panics on arbitrary
  input, and determinism. Do not write properties that reproduce the
  implementation.
- Critical paths—authorization, durable writes, quotas/allocations, solver
  correctness, state transitions, identifiers/paths, irreversible actions, and
  project input parsing—need direct unit coverage, negative/boundary cases,
  regression fixtures, property tests where meaningful, and boundary
  integration coverage.
- Keep pure helper tests in `#[cfg(test)]` beside the behavior. Put cross-route
  or server behavior in `tests/` submodules with shared setup. CLI contract
  tests should exercise parsed commands and request construction.
- Use `#[tokio::test]` for async behavior. Test timers and timeouts with paused
  time, injected clocks, latches/channels, or tiny deterministic profiles, not
  real sleeps. Prove scheduling properties directly: blocking work does not
  starve timers, limits cap entrants, and cancellation prevents side effects.
- Use fakes, mocks, or local test servers only at external boundaries. Do not
  mock pure functions or internal implementation details. Keep the failure mode
  visible and controllable.
- Snapshot tests fit stable diagnostics, generated text, CLI help, and stable
  serialized output. Review every change; do not accept broad snapshot updates
  without explaining each output difference. Prefer structured assertions when
  only a few fields matter.
- Tests must not implicitly depend on wall-clock time, network access, process
  scheduling, random seeds, local timezone, machine paths, global environment,
  or execution order. Centralize and serialize test-only process-global state,
  especially environment mutation.
- Prefer structured fixtures under `fixtures/` over inline walls of data. Name
  fixtures after the behavior they cover. Do not commit generated outputs,
  credentials, local run roots, or machine-specific artifacts.
- Validate operational YAML and deployment files where possible. Configuration
  that disables auth, changes trust boundaries, or touches secrets needs
  explicit regression coverage.

## 9. Observability, Performance, and Dependencies

- Prefer structured `tracing` fields to interpolated prose. Use `error` for
  failures requiring attention, `warn` for degraded/suspicious behavior, `info`
  for meaningful lifecycle/business events, and `debug`/`trace` for diagnostic
  detail. Do not use `error` for expected user-input failures unless the
  application context calls for it.
- Measure before optimizing. Use representative profiling, benchmarks, or
  production-like measurements before adding non-obvious complexity. Benchmark
  critical algorithms with Criterion or an equivalent approach.
- Review for accidental quadratic behavior: repeated linear searches, repeated
  string concatenation, front insertion into vectors, rescanning, repeated
  sorting, and nested iteration over large inputs. Optimize allocation or
  borrowing only when measurement shows it matters.
- Add a dependency only when it clearly beats the standard library or a small
  local implementation. Consider maintenance, adoption, license, transitive
  cost, build time, security history, feature granularity, and MSRV.
- Any dependency addition or major upgrade must state why it is needed, why the
  selected crate fits, needed feature flags, and security or portability impact.
  Do not add overlapping crates without a documented architectural reason.

## 10. Documentation, Linting, and Verification

- Update Rustdoc, module docs, examples, README/DEVELOPMENT guidance, CLI help,
  configuration references, and deployment notes whenever behavior changes.
  Outdated documentation is a defect.
- Do not suppress Clippy warnings without understanding them. A suppression must
  be narrow, justified when not self-evident, and not conceal a design issue.
  Prefer the underlying fix. Configure useful lints centrally where practical.
- First run the smallest meaningful validation, then the broader suite. For a
  normal Rust change, run the applicable commands from:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features --locked
cargo test --doc --workspace
```

- Where the repository supports them, also run `cargo nextest run`, `cargo
  audit`, `cargo deny check`, Miri for unsafe/aliasing-sensitive code, fuzzing
  for structurally complex external input, and broader property-test case counts
  in scheduled or pre-release work.
- CI should run formatting, all-target compilation, Clippy with warnings denied,
  unit/integration/doc/property tests, and applicable dependency/security
  policy checks. Main or scheduled CI should additionally cover supported
  platforms, meaningful feature combinations (including no-default-features
  when intended), MSRV, Miri/sanitizers, fuzzing, ignored expensive tests, and
  performance regression checks.
- Do not claim verification that was not run. Distinguish failures in tests from
  environment, credentials, or infrastructure failures, and state the coverage
  gap honestly.

## 11. Change Discipline and Completion

Before editing, inspect the affected modules, tests, public APIs, invariants,
side effects, and repository-specific instructions. Match local conventions
when they are adequate; do not introduce a new architectural pattern merely for
consistency with a generic rule.

For each change:

1. Define expected inputs, outputs, failure modes, invariants, compatibility
   requirements, and test strategy.
2. Design the smallest coherent change with pure domain logic, typed errors,
   deterministic behavior, isolated I/O, and bounded async work.
3. Add or update tests alongside the implementation; add property tests for new
   critical-path logic and a targeted regression for defect fixes.
4. Review the diff for accidental API changes, unnecessary clones, hidden
   blocking, discarded errors, nondeterminism, missing tests, stale comments,
   new dependencies, unsafe code, and files that need splitting.
5. Run and accurately report the relevant validation.

A change is complete only when the behavior and documentation are updated,
relevant tests exist and pass, critical invariants have appropriate coverage,
errors are actionable, async work is bounded/cancellation-aware where relevant,
and no unnecessary unsafe code or unrelated refactoring was introduced.

## 12. Prohibited Patterns

Do not introduce these without explicit, documented justification:

- production `unwrap`/`expect` on fallible input, ignored `Result`s, opaque
  string errors in reusable APIs, or silent fallbacks that conceal invalid input
- unbounded task spawning, blocking an async executor, locks held across
  `.await`, or detached work with no ownership decision
- undocumented unsafe blocks, global mutable state, hash iteration-order
  dependence, or shell command construction from untrusted input
- broad lint suppressions, nondeterministic tests, assertions about irrelevant
  implementation details, or mocks for pure functions
- large multi-purpose modules, speculative traits/frameworks, dead code,
  stale TODOs, or unrelated refactoring in a targeted change

When several implementations are viable, choose the one that uses safe Rust,
keeps domain logic pure, makes failure explicit, preserves determinism,
minimizes shared mutable state, keeps async orchestration at the boundary, has a
small deliberate public API, and is easy to understand and test.
