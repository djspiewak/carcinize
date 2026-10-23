# GitLab protocol boundary

The compatibility reference is GitLab Runner **17.7.1**, commit
[`6669abcf396dae10a4b820aa7ad053cbb6b286cc`](https://gitlab.com/gitlab-org/gitlab-runner/-/tree/6669abcf396dae10a4b820aa7ad053cbb6b286cc).
Relevant upstream files are `common/network.go`, `network/gitlab.go`,
`network/trace.go`, `network/patch_response.go`, and
`network/remote_job_state_response.go`. Server serialization and native step
semantics follow GitLab 17.7.7's `lib/api/entities/ci/job_request/`,
`lib/api/ci/runner.rb`, and
[`run_steps.json`](https://gitlab.com/gitlab-org/gitlab/-/blob/v17.7.7-ee/app/validators/json_schemas/run_steps.json).

## Registration and ownership

All endpoints are constructed under the configured HTTPS base URL, preserving
deployment prefixes. The administrative token resolves the encoded project path,
creates a `project_type` runner through `POST /api/v4/user/runners`, and deletes
that runner through `DELETE /api/v4/runners/:id`. Tags are explicit, untagged
jobs are disabled, and the runner is locked to the project.

Deletion is idempotent: 204/404 confirm removal; transient failures allow up
to four total attempts with 2/4/6-second backoffs. With the 30-second request
timeout, runner deletion is bounded by 132 seconds in addition to earlier job
cleanup. Exhaustion returns an error. Creation is never blindly retried.
Shutdown during registration waits for the bounded request to recover the
identity and remove it. Each invocation creates
a random system ID and verifies the new runner token before polling. A malformed
response with a known runner ID still triggers deletion. No credentials or runner
identity files are persisted. Legacy registration tokens and attaching to an
existing runner are outside scope.

## Job lifecycle

`POST /api/v4/jobs/request` uses the runner token, system ID, protocol metadata,
and queue cursor. HTTP 201 assigns a job; 204 and 409 return to polling. Idle
polls wait three seconds; transient failures have three bounded retries. Long
polls have a 90-second total timeout; ordinary requests have 30-second total and
10-second connection timeouts. Redirects are disabled.

Acquisition is not cancelled by dropping its future on Ctrl-C: an assignment
already made by the server must be received and cancelled. Assignments must
belong to the resolved project. The administrative token is never sent to an
endpoint supplied by a job, action, image, dependency, or repository declaration.

Job tokens authenticate trace PATCH requests and running/terminal PUT updates.
Heartbeats run during preparation and while trace transmission is pending.
Preparation runs on one owned blocking worker per assignment; cancellation
joins that worker before releasing the assignment. Inspection, including
preparation, is bounded by the dispatched job timeout (capped at 24 hours).
Remote cancellation interrupts inspection. Local Ctrl-C calls the project cancellation endpoint for only the
current job, then acknowledges cancellation through the runner protocol.
Terminal `canceling` transitions become `canceled` when acknowledged with
`failed` and `job_canceled`; `canceled` is not sent as a runner update state.

Finalization has a 120-second bound. A 202 response is pending, not successful.
Shutdown remains active while completion is pending; the runner cancels only
its current job and finishes the cancellation acknowledgment. An interrupted
PATCH retains its local bytes for range and checksum reconciliation. A failed
completion triggers cancellation and a second cleanup/acknowledgment window of
at most 120 seconds; cancellation requests retain the normal 30-second HTTP
bound. Total shutdown time can therefore exceed the first finalization window.
Revoked job tokens after cancellation or a lost completion acknowledgment cause
an administrative project-job lookup to confirm the actual outcome. Unknown
outcomes and cleanup failures return errors. Forced termination or a lost
registration/acquisition response can still leave server-side state; durable
recovery and runner token rotation are not implemented.

## Declarations and traces

The credential envelope is decoded separately from declarations. Standard
scripts, release/after-script phases, hooks, variables, images, services, caches,
artifacts, and dependency metadata have typed representations. Native `run`
JSON supports scripts, reusable step references (string or Git object), action
references, `run`/legacy `steps` sequences, executable commands, environment,
inputs, outputs, and delegation. Native numeric spellings are checked against
the rendered decimal value; numbers that would be rounded fail inspection
explicitly. Equivalent spellings are accepted when the numeric value is
preserved. The shared JSON parser retains its existing feature set and object
behavior. Numeric spellings count toward preparation work limits. Nesting is
bounded. Unknown semantic variants fail inspection; external secret-provider
declarations fail explicitly. Nullable
variable values are preserved as null. Masking checks decoded native JSON and
scalar values. Sensitivity flags apply only to actual job/image/service
variables; arbitrary native input/output records retain their data. Colliding
redacted mapping keys fail inspection instead of silently dropping declarations.

Conditions, references, and expressions are never evaluated. No checkout,
artifact/cache transfer, registry access, external secret resolution, process,
container, or action execution occurs. GitLab has already selected the job and
merged `before_script` with `script`. The runner cannot reconstruct original
YAML or declarations the server omitted.

Each trace starts with the inspection-only banner and contains generated
collapsible sections. Administrative/runner/job tokens, dependency and registry
credentials, masked/private/file variables, and credential-bearing URL values
are masked before JSON escaping, byte chunking, and checksumming. Raw job
envelopes and remote error bodies are never diagnostic text. Declaration text
cannot introduce trace section control characters.

Responses and rendered traces are bounded to 8 MiB. Patches use up to 64 KiB,
with inclusive byte `Content-Range` values. GitLab's returned `Range: 0-N`
represents the next byte offset, not an inclusive end. Reconciliation resends
only the necessary bytes after a lost acknowledgment. Final PUT requests include
the acknowledged trace length and CRC32 IEEE checksum. `crc32fast` supplies this
protocol checksum with its default standard-library feature and no additional
features; it is not used for authentication or cryptography.

Preparation also has independent complexity limits: each outer/native JSON
document has at most 8,192 values and depth 48; masking accepts at most 384
expanded literal entries and 32 KiB of secret strings. A conservative 64 MiB
scan budget multiplies declaration text/key/numeric bytes plus 32 bytes per JSON value
by the expanded masking-entry count and a factor of three for replacement
growth. Exceeding these bounds fails inspection explicitly. Mask matches refer
only to original text; secrets containing `*`, `**`, or `***` are supported without
recursively expanding inserted markers. Preparation, structured masking, bounded
serialization, outcome accounting, and work/liveness/shutdown supervision are
shared with GitHub. GitLab retains its credential discovery, native numeric audit,
trace framing, and completion/reconciliation state machine. Shutdown-signal errors
survive successful finalization and cleanup. These bounds keep preparation finite
without changing the 8 MiB response/trace ceilings.

Offline tests use local HTTP fakes and synthetic credentials. They verify
declaration preservation, non-execution, masking, prefixes/project encoding,
registration ownership, multiple jobs, trace offsets/retries/checksums,
pending completion, cancellation, and failure cleanup.
