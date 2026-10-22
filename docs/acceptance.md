# Interoperability evidence

Verified against GitHub.com on 2026-10-22 using the locally built Rust release
binary on macOS ARM64. No GitHub-provided runner executable was involved.
The implementation used for the successful experiments is commit `b2f8a8e`.

## Real workflow results

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

## Verification

`make verify` passed: formatting, all-target compilation, Clippy with warnings
denied, all **26 tests**, and the documentation-test gate. Ordinary tests use
local HTTP/WebSocket fakes and need no GitHub credentials. Both workflow files
passed `actionlint`; the local release build passed.

Initial hosted Linux and macOS verification and release builds passed in
[CI run 39819537626](https://github.com/djspiewak/carcinize/actions/runs/39819537626).
Both platforms also passed verification, release builds, and artifact upload for
the protocol-fix commit in
[CI run 39824245120](https://github.com/djspiewak/carcinize/actions/runs/39824245120).

## Observed protocol details and limits

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
