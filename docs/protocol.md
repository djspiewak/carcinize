# Protocol boundary

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
Redirects are disabled, service URLs require HTTPS, and error messages exclude
remote bodies and URL query strings.

The template decoder retains scalar types, ordered mappings (including expression
keys), sequences, expression and insert nodes, and source coordinates. Script input
fields become typed script/shell/working-directory fields. Credentials are decoded
separately so malformed declarations can still receive a failed completion.

The Results API's status/conclusion values differ from the run-service completion
values. Their encoders are intentionally separate. For example, completed/success
is 6/2 in Results and 2/0 in completion step results.

## Dependencies

- `tokio` owns asynchronous I/O, timers, signals, and task lifetimes.
- `reqwest` uses Rustls, JSON support, explicit timeouts, and no redirect following.
  `tokio-tungstenite` supplies the authenticated live WebSocket feed.
- `serde`/`serde_json` implement typed wire and declaration serialization;
  `url` constructs service endpoints and query parameters.
- `rsa` generates ephemeral RSA keys. `jsonwebtoken` delegates PS256 signing to
  ring from DER keys; PEM parsing is disabled. `rand` supplies OS randomness,
  `base64` encodes public-key components, and `uuid` supplies invocation identities.
- `chrono` formats protocol timestamps. `regex` implements supplied secret-mask
  hints; unsupported hints fail closed.
- `clap` handles the CLI; `thiserror` preserves typed internal failures;
  `anyhow` adds context at the binary boundary.
- Test-only `axum`, `tempfile`, and `proptest` provide local boundary fakes,
  non-execution sentinels, and parser properties.

No process-execution, container, action-download, YAML-workflow interpreter, or
GitHub runner dependency is present. Keep Cargo.lock committed.
