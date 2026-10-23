# Interoperability evidence

## GitLab Pipelines

Verified on 2026-10-22 against a provisioned GitLab **17.7.7-ee** server using
GitLab Runner **17.7.1** as the protocol reference. The local macOS ARM64
release binary was built from the implementation before its merge into `main`.
No GitLab Runner executable was involved. Host selection was explicit and did
not use remote inference. Deployment-specific hostnames, project paths, IDs,
and private run URLs are intentionally omitted from this repository.

### Successful acceptance

Four pipelines completed on **one invocation** with tag `carcinize`:

| Experiment | Outcome | Archived trace |
| --- | --- | --- |
| Minimal | Success | 2 sections, 17,667 bytes, including `exit 97`. |
| Semantic | Success | 4 sections, 19,600 bytes. |
| Native steps | Success | 4 sections, 18,463 bytes. |
| Semantic repeated | Success | 4 sections, 19,600 bytes. |

The jobs API confirmed that all four jobs used the same runner and source
revision. Runner details confirmed `locked: true`, `run_untagged: false`,
tag `carcinize`, and association only with the selected project. Archived traces
were fetched through `glab api`, checked for balanced sections, and their JSON
declarations decoded. All four traces masked four sensitive variables, masked
repository URL credentials, and omitted the administrative token.

Semantic traces retained the merged `before_script`, pre-source hook, script
and after-script declarations (`exit 96`, `exit 97`, `exit 99`), Unicode
`雪 café`, unavailable image/service references, missing cache/artifact paths,
and a missing working directory. Native traces preserved the unavailable
reusable-step reference, integer and boolean inputs, expressions referring to
nonexistent outputs, and a nested executable declaration whose program does
not exist. These successful inspections do not certify normal pipeline execution.

Ctrl-C exited with status zero and:

```json
{"jobs_inspected":4,"jobs_failed":0,"jobs_cancelled":0}
```

A subsequent project runner listing was `[]`, confirming removal.

### Reproduction commands

Supply `GITLAB_TOKEN` through the process environment. Set `GITLAB_HOST` to the
explicit server hostname and `GITLAB_PROJECT` to the namespaced project path.
The following commands use deployment parameters rather than recorded private
identifiers. The experiment configuration must be present on the selected branch.

```sh
make verify
cargo build --release --locked
./target/release/carcinize run --provider gitlab \
  --host "$GITLAB_HOST" --repo "$GITLAB_PROJECT" \
  --label carcinize --name carcinize-acceptance
```

In another terminal, dispatch `minimal`, `semantic`, `native`, and `semantic`
in sequence using the same runner invocation:

```sh
glab ci run --repo "https://$GITLAB_HOST/$GITLAB_PROJECT" \
  --branch main --variables CARCINIZE_EXAMPLE:minimal
```

Set the following ID variables from the selected deployment's API responses:

```sh
glab api --hostname "$GITLAB_HOST" "runners/$RUNNER_ID"
glab api --hostname "$GITLAB_HOST" "projects/$PROJECT_ID/pipelines/$PIPELINE_ID/jobs"
glab api --hostname "$GITLAB_HOST" "projects/$PROJECT_ID/jobs/$JOB_ID/trace"
glab api --hostname "$GITLAB_HOST" "projects/$PROJECT_ID/runners"
```

`make verify` passed formatting, all-target compilation, Clippy with warnings
denied, **74 library tests and 1 CLI test**, and the documentation-test gate
(zero documentation tests). The original GitHub tests remain present and pass.
The release build passed. The server's CI lint API accepted the experiment
configuration without errors or warnings. GitLab acceptance did not dispatch
GitHub workflows.

### Verification limits and unsuccessful probes

- Live acceptance covered normal completion, archived declarations, repeated
  jobs, runner assignment, and idle Ctrl-C cleanup. Browser rendering of the
  live trace and collapsible sections was not separately observed.
- Cancellation during acquisition/preparation/upload/finalization, remote
  cancellation, transient failures, range/checksum repair, pending 202
  completion, malformed payloads, cleanup failures, response/log/workload
  limits, and escaped/scalar credential masking use offline boundary or pure
  tests. These cases were not all induced on the live server.
- Host prefixes, custom ports, nested project paths, and native action/Git-object
  reference variants have offline coverage; live acceptance used one server,
  a two-component project path, and the native experiment declarations.
- An early probe exposed valid null variable values that the initial decoder
  rejected. Its runner was removed and its job canceled through the project
  API. Nullable variables and malformed-envelope cleanup have regression tests.
- An earlier invocation completed four inspections but encountered DNS failures
  during polling and deletion. It returned an error and required explicit
  removal of its runner. Subsequent invocations completed normal Ctrl-C cleanup.
  Transient deletion recovery is tested offline; the DNS outage was not
  reproduced against the final binary.
- Abrupt process termination or an ambiguous creation/assignment response can
  leave provider state. Persistent recovery is outside scope. Known unresolved
  reporting/cleanup failures return errors rather than successful summaries.
  See [gitlab-protocol.md](gitlab-protocol.md) for pinned protocol references
  and precise resource and shutdown bounds.

## GitHub Actions (existing acceptance)

Verified against GitHub.com on 2026-10-22 using the locally built Rust release
binary on macOS ARM64. No GitHub-provided runner executable was involved.
The implementation used for the successful experiments is commit `b2f8a8e`.

### Real workflow results

All three successful jobs below ran on **one invocation**, registered as runner
ID **8**, `carcinize-0c6e6467-47ca-429f-a6c1-aae50e74b5c7`, with labels
`self-hosted`, `OSX`, `ARM64`, and `carcinize`.

| Experiment | GitHub run | Observed result |
| --- | --- | --- |
| Minimal round trip | [39824129393](https://github.com/djspiewak/carcinize/actions/runs/39824129393) | One distinct successful step; archived declaration includes `exit 97`. |
| Semantic inspection, first dispatch | [39824259184](https://github.com/djspiewak/carcinize/actions/runs/39824259184) | Five distinct successful steps, including the false conditional. |
| Semantic inspection, second dispatch | [39824356108](https://github.com/djspiewak/carcinize/actions/runs/39824356108) | Same runner ID, another five successful steps and complete downloadable logs. |

The minimal workflow references the initial workflow commit `8688e5f`; the
laptop binary already included the protocol fixes subsequently committed in
`b2f8a8e`. Both semantic dispatches reference `b2f8a8e`.

GitHub's jobs API confirmed runner assignment, step order, names, and conclusions.
`gh run view --log` returned the inspection output. The second semantic run's
downloaded ZIP contained five individual step logs, the combined job log, and
GitHub's `system.txt`. Each individual step log was present in the combined log.
The logs preserve:

- Multiline scripts, `exit 97`/`exit 99`, a nonexistent working directory,
  timeout, and continue-on-error declarations.
- Repository, local, and container action references and inputs.
- Job environment and defaults, step environment, expression nodes, the
  nonexistent step output reference, and supplied file/line/column coordinates.
- The false step condition, which was inspected successfully without evaluation.

The successful reporting path established an authenticated live-feed WebSocket,
sent step output before completion, and finalized Results blobs and metadata.
The local boundary tests independently assert live-feed delivery before
completion. Browser rendering of the transient live stream was not separately
observed; archived output and individual step records were verified through
GitHub's APIs.

Ctrl-C exited successfully with
`{"jobs_inspected":3,"jobs_failed":0,"jobs_cancelled":0}`. A subsequent runner
listing confirmed that ID 8 had been removed. An earlier registration-only
probe also verified online status and removal of its own registration (ID 2).
After the failed probes' assignments cleared, their registrations were removed
as well; GitHub's repository runner list was empty at the end of acceptance.

### Verification

`make verify` passed: formatting, all-target compilation, Clippy with warnings
denied, all **26 tests**, and the documentation-test gate. Ordinary tests use
local HTTP/WebSocket fakes and need no GitHub credentials. Both workflow files
passed `actionlint`; the local release build passed.

Initial hosted Linux and macOS verification and release builds passed in
[CI run 39819537626](https://github.com/djspiewak/carcinize/actions/runs/39819537626).
Both platforms also passed verification, release builds, and artifact upload for
the protocol-fix commit in
[CI run 39824245120](https://github.com/djspiewak/carcinize/actions/runs/39824245120).

### Observed protocol details and limits

- This repository uses tenant registration (`use_v2_flow` absent), followed by
  the modern broker, run-service, and Results protocols.
- GitHub sent named enums and an already formed WebSocket feed URL. Local
  fixtures now cover named/numeric enums and WebSocket URLs.
- Run-service completion requires the VSS formatter's named enums, distinct
  from the Results protobuf enum numbers. Failure annotations use `level`.
- GitHub may normalize expressions and conditions before assignment. The binary
  preserves what the runner protocol supplies, not the original YAML text.
- Early, unsuccessful protocol probes were cancelled. GitHub temporarily
  refused registration deletion with HTTP 422 while those abandoned assignments
  still counted as busy; their leases must clear before deletion. This is a
  failure-recovery limitation, distinct from the successful Ctrl-C cleanup.
- Cancellation, lease loss, OAuth refresh, and reporting failures were exercised
  through local boundary fakes; live acceptance covered normal completion and
  idle Ctrl-C. See [protocol.md](protocol.md) for the pinned source reference.
