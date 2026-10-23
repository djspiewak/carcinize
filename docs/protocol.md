# Protocol boundary

This document covers GitHub Actions. See [gitlab-protocol.md](gitlab-protocol.md)
for the GitLab provider, its pinned reference, and separate reporting protocol.

Reference: official runner v2.338.1, commit
`e78522a9a7b01c3bcd7ac0da6e3a9bb6cd513a76`.

The implementation follows these upstream boundaries:

- `Runner.Listener/Configuration/ConfigurationManager.cs`,
  `CredentialManager.cs`, and `Runner.Common/RunnerDotcomServer.cs`: registration
  token, protocol discovery, default group, public RSA key registration. The
  bootstrap's `use_v2_flow` flag selects runner-admin registration. Without it,
  registration uses the returned tenant token and default pool, then obtains the
  modern broker URL from the registered agent's `ServerUrlV2` property.
- `Sdk/WebApi/WebApi/OAuth/` and `VssSigningCredentials.cs`: PS256 client assertion
  and OAuth client-credentials exchange.
- `Runner.Listener/BrokerMessageListener.cs`, `RunnerJobRequestRef.cs`, and
  `Sdk/WebApi/WebApi/BrokerHttpClient.cs`: broker sessions, polling, optional
  acknowledgment, and cancellation.
- `Sdk/RSWebApi/RunServiceHttpClient.cs`: acquisition, renewal, and completion.
  Acquisition uses the assignment URL and runner token. Renewal and completion
  use the job's SystemVssConnection URL and job token.
- `Sdk/DTPipelines/Pipelines/` and
  `Sdk/DTObjectTemplating/ObjectTemplating/Tokens/`: structured steps and templates.
- `Sdk/WebApi/WebApi/ResultsHttpClient.cs`, `Contracts.cs`, and
  `Runner.Common/ResultsServer.cs`: step status, signed blob URLs, log metadata,
  and the live console feed.

All paths above are relative to the reference's
[`src/` directory](https://github.com/actions/runner/tree/e78522a9a7b01c3bcd7ac0da6e3a9bb6cd513a76/src).

Credentials have separate scopes: the administrative token creates and removes
registrations; RemoteAuth registration tokens perform bootstrap and runner-admin
registration, or a temporary tenant token performs tenant registration;
the RSA identity obtains runner tokens for broker/acquisition requests; job tokens
authenticate reporting and renewal. Signed storage URLs receive no bearer header.
Redirects are disabled, service URLs require HTTPS (WSS for the live feed), and error messages exclude
remote bodies and URL query strings.

The template decoder retains scalar types, ordered mappings (including expression
keys), sequences, expression and insert nodes, and source coordinates. Script input
fields become typed script/shell/working-directory fields. Credentials are decoded
separately so malformed declarations can still receive a failed completion.
Step/action discriminants and mask hints accept both numeric and named enums,
as the upstream JSON converters do. Unknown values fail explicitly.

The Results API's protobuf enums differ from the run-service completion enums.
Their encoders are intentionally separate. For example, completed/success is
6/2 in Results; the run service uses the VSS formatter's camel-case enum names,
`completed`/`succeeded`. Failure annotations use `level: "failure"`.

## Shared inspection and lifecycle policy

The provider captures registration and minimal job cleanup identities before
validating ancillary authorization, declaration, or masking metadata. Malformed
metadata with a usable identity therefore still triggers deletion or failed job
completion. Registration is never retried; deletion of the owned runner permits
four total attempts with 2/4/6-second backoffs (at most 132 seconds with request
timeouts). Lost responses without usable identity still require manual recovery.

Declaration preparation runs on one owned blocking worker, joined on interruption.
The shared preparation validator admits at most 8,192 values, depth 48, and 8 MiB
of text/key/numeric data per document. Mask construction admits at most 384 expanded
entries and 32 KiB of literal/pattern source metadata; each regex has a 256 KiB
compiled-size limit. A shared 64 Mi-unit work budget charges literal scans and
output plus every regex search, including repeated suffix scans. Regex charges
use compiled NFA memory times remaining input bytes, with a factor of two for
iterator advances that skip empty matches. Each advance is capped at 4 Mi units;
cancellation is checked between advances and values. These conservative limits
can reject large inputs even when a particular regex would match quickly.
PikeVM retains leftmost-first, greedy matching; budget exhaustion fails inspection.
Rendered declarations are checked again before masking and bounded serialization.

Shared rendering masks decoded values, scalar spellings, and mapping keys before
JSON escaping. Key collisions fail inspection. Matches refer only to original
text, so literal or regex matches cannot repeatedly expand inserted `***` markers.
GitHub templates and their source coordinates remain provider-specific; no common
job schema replaces them. Timestamps are supplied to pure preparation.

An initial lease renewal precedes preparation. One liveness future then retains
renewal timing and broker cancellation across preparation, step reporting, combined
log publication, and terminal acknowledgment. Final log publication is drained
before choosing the completion outcome, so cancellation or lease loss during that
publication cannot submit success. Local shutdown keeps renewal active while a
final write drains. Once a terminal request is in flight, its acknowledgment is
retained; shutdown cannot retroactively change a confirmed outcome. Operational
shutdown errors are returned after cleanup even if reporting succeeds.

Shared supervision imposes a 24-hour preparation/inspection safety deadline.
Final HTTP writes and cleanup remain owned and use their bounded request timeouts;
they may extend beyond the inspection deadline. Tests use local protocol fakes,
shared behavioral assertions, and deterministic supervisor/worker tests.

## Dependencies

- `tokio` owns asynchronous I/O, timers, signals, and task lifetimes.
- `reqwest` uses Rustls, JSON support, explicit timeouts, and no redirect following.
  `tokio-tungstenite` supplies the authenticated live WebSocket feed.
- `serde`/`serde_json` implement typed wire and declaration serialization;
  `url` constructs service endpoints and query parameters.
- `rsa` generates ephemeral RSA keys. `jsonwebtoken` delegates PS256 signing to
  ring from DER keys; PEM parsing is disabled. `rand` supplies OS randomness,
  `base64` encodes public-key components, and `uuid` supplies invocation identities.
- `chrono` formats protocol timestamps. `regex-automata` implements bounded
  supplied secret-mask hints; `regex` recognizes fixed URL credential patterns.
  Unsupported hints fail closed.
- `clap` handles the CLI; `thiserror` preserves typed internal failures;
  `anyhow` adds context at the binary boundary.
- Test-only `axum`, `tempfile`, and `proptest` provide local boundary fakes,
  non-execution sentinels, and parser properties.

No process-execution, container, action-download, YAML-workflow interpreter, or
GitHub runner dependency is present. Keep Cargo.lock committed.
