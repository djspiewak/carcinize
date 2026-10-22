# carcinize

A standalone, inspection-only GitHub Actions runner written in Rust. It registers
with GitHub, receives structured jobs, and reports each declaration as a separate
step with live and archived logs. It does not invoke GitHub's runner binary.

**Success means inspection and reporting succeeded. Nothing in the workflow is
executed.** Conditions and expressions are preserved, including conditions that
would skip a step on a normal runner. Action references are retained without
fetching manifests, downloading actions, checking out source, or starting containers.

## Run on a laptop

Build with stable Rust:

```sh
cargo build --release --locked
./target/release/carcinize run --repo djspiewak/carcinize --label carcinize
```

Set `GITHUB_TOKEN` in the process environment before launching. Use a token that
can administer runners on the repository. For a fine-grained token this requires
repository **Administration: write**; a classic token requires the appropriate
`repo` scope and repository administrator access. Do not put the token in command
arguments, workflow files, or logs.

Repeat `--label` for additional scheduling labels. If omitted, the custom label is
`carcinize`. Standard `self-hosted`, host OS, and architecture labels are also
registered. `--name` sets a display name; the default is unique per invocation.
An existing runner is never replaced.

The runner processes jobs sequentially until Ctrl-C, then removes its own session
and repository registration. Credentials and RSA identity remain in memory.
Progress goes to stderr; a final JSON count goes to stdout. Forced termination,
power loss, or an ambiguous registration response can leave a runner registration;
remove that specific entry through repository Settings → Actions → Runners.

## GitHub experiment

The `CI` workflow uses ordinary GitHub-hosted Linux and macOS runners to verify
and build the project. It needs no laptop runner or administrative credential.

With carcinize listening, manually dispatch the separate experiment workflow:

```sh
gh workflow run inspect.yml -f example=minimal -f runner_label=carcinize
gh workflow run inspect.yml -f example=semantic -f runner_label=carcinize
```

The minimal probe contains one script. The semantic example includes script,
repository-action, local-action, and container-action declarations, inputs,
environment, defaults, conditions, and expressions. Scripts intentionally contain
failing commands. A green inspection run demonstrates that they were described,
not executed. A local action manifest exists only to make the example reference
concrete; carcinize does not resolve it.

Use the run page to inspect individual steps and live output, then download the
logs or use `gh run view RUN_ID --log`. Dispatch the semantic example twice without
restarting carcinize to check that one registration accepts multiple jobs.

## Development

```sh
make verify
```

This checks formatting, all-target compilation, Clippy with warnings denied, unit
and boundary tests, and documentation tests. Tests use synthetic inputs and local
HTTP/WebSocket servers; they need no GitHub access or credentials. They cover
semantic preservation, registration and signed authentication, job lifecycle,
non-execution, redaction, live and archived logs, failure, and cleanup.

Domain declarations are independent of protocol credentials. The model decoder
and redactor are synchronous; registration, broker, job context, and reporting
modules own their I/O. The binary entry point only parses arguments and wires the
library to the shutdown signal.

## Compatibility and limits

This PoC targets repository runners on GitHub.com. It supports the runner-admin
and tenant registration handshakes, then uses the modern broker, run-service,
and Results protocols, using
[`actions/runner` v2.338.1](https://github.com/actions/runner/tree/e78522a9a7b01c3bcd7ac0da6e3a9bb6cd513a76/src)
as its compatibility reference. GitHub does not publish a stable runner SDK
contract. The reported compatibility version is fixed; automatic runner updates
are disabled. Version rejection requires checking the upstream protocol and
updating this implementation.

Legacy job protocols, GHES, action execution, manifest expansion, expression
evaluation, persistent recovery, and deployments are outside scope. Unsupported
semantic discriminants fail inspection instead of becoming opaque strings.
The server may already have transformed expressions before dispatch; the PoC
preserves the declarations it receives, not the original workflow YAML verbatim.

Responses and combined inspection logs are each limited to 8 MiB. Logs are masked
before splitting or transport. Full credential-bearing job envelopes are never
logged. Completed step logs and the combined job log use final blob uploads; the
WebSocket feed provides output while the job is active. A cancelled in-flight step
can have its partial output in the combined archive without a completed step blob.

Protocol details and dependency choices are in [docs/protocol.md](docs/protocol.md).
Successful live run URLs and verification limits are recorded in
[docs/acceptance.md](docs/acceptance.md).
