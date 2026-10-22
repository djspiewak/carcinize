# Interoperability evidence

Local boundary tests establish behavior against the pinned protocol fixtures.
They do not establish interoperability with GitHub.

On 2026-10-22, the native binary registered as repository runner ID 2 with
`self-hosted`, `OSX`, `ARM64`, and `carcinize` labels. GitHub reported it online.
After Ctrl-C, GitHub's repository runner list was empty, confirming cleanup.
The repository currently uses tenant registration followed by the broker
protocol; see [the protocol notes](protocol.md).

All 24 local tests, `make verify`, the release build, and `actionlint` passed.

Workflow acceptance is pending publication. Record actual run URLs after the
minimal probe and two semantic dispatches on a single registration, checking
assignment, per-step records, live output, archived logs, and non-execution.
