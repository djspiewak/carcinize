# carcinize

A standalone, inspection-only GitHub Actions and GitLab Pipeline runner written
in Rust. It registers with the selected provider, receives structured jobs, and
reports declarations with live and archived logs. It does not invoke either
provider's runner binary. GitHub uses individual step records; GitLab uses job
trace sections.

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

## GitLab on a laptop

Set `GITLAB_TOKEN` in the process environment to an access token with `api` access
and permission to create and manage project runners. On GitLab 17.7, creation
requires project Owner access; scoped tokens also need `create_runner` and
`manage_runner`. Keep tokens out of command arguments and logs.

```sh
./target/release/carcinize run --provider gitlab \
  --host gitlab.example.com --repo group/subgroup/project --label carcinize
```

`--host` is required and is never inferred from Git remotes. It accepts a
hostname with an optional port or an HTTPS base URL, including a deployment
prefix such as `https://gitlab.example.com/gitlab`. HTTP, embedded credentials,
queries, and fragments are rejected. GitHub remains the default provider and
does not accept `--host`.

GitLab labels become runner tags. The transient runner is locked to the selected
project and does not accept untagged jobs. It processes jobs sequentially until
Ctrl-C, cancels its active job if necessary, and deletes its own registration.
Runner credentials stay in memory; an existing runner is never replaced.

The GitLab experiment pipeline starts only through the API or the web UI:

```sh
glab ci run --repo https://gitlab.example.com/group/subgroup/project \
  --branch main --variables CARCINIZE_EXAMPLE:minimal
glab ci run --repo https://gitlab.example.com/group/subgroup/project \
  --branch main --variables CARCINIZE_EXAMPLE:semantic
glab ci run --repo https://gitlab.example.com/group/subgroup/project \
  --branch main --variables CARCINIZE_EXAMPLE:native
```

Use `CARCINIZE_RUNNER_TAG` to select a different tag. Repeat the semantic example
without restarting the runner to test multiple assignments. Inspect the job
trace in GitLab or with `glab ci trace JOB_ID`. Deliberate failing commands and
unavailable images/step references must appear in successful inspection logs.
These pipelines do not build or test the project; run `make verify` locally.

GitLab inspection preserves the dispatched declaration, including native `run:`
steps, inputs, environments, nested sequences, action references, and executable
declarations. It never resolves or executes them. GitLab merges `before_script`
into the dispatched script and evaluates pipeline rules before assignment; the
original YAML and skipped jobs cannot be reconstructed from that response.

## Development

```sh
make verify
```

This checks formatting, all-target compilation, Clippy with warnings denied, unit
and boundary tests, and documentation tests. Tests use synthetic inputs and local
HTTP/WebSocket servers; they need no provider access or real credentials. They cover
semantic preservation, registration and signed authentication, job lifecycle,
non-execution, redaction, live and archived logs, failure, and cleanup.

Domain declarations are independent of protocol credentials. Provider-specific
models and synchronous rendering remain separate from network orchestration.
The providers share configuration dispatch, summaries, HTTP infrastructure, and
redaction. The binary entry point only parses arguments and wires the library
to the shutdown signal. `Config::new` retains GitHub behavior;
`Config::new_gitlab(host, project, labels, name, token)` selects GitLab through
the same `run` function and JSON summary.

## Compatibility and limits

The GitHub provider targets repository runners on GitHub.com. It supports the runner-admin
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

The GitLab provider follows Runner 17.7.1 and targets GitLab 17.7-compatible
servers through an explicit host. Protocol behavior, masking, cancellation,
cleanup bounds, and unsupported external secret-provider declarations are
documented in [docs/gitlab-protocol.md](docs/gitlab-protocol.md). A crashed
process or an ambiguous registration/assignment response can leave server-side
state; inspect and remove only that invocation's runner in project CI/CD settings.
