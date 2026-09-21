# Advisory decision support

Decision support lets an operator ask Jev for a suggested review owner from an
excerpt the current OMAR run has already produced. It is advisory: it cannot
write workflow ports, send an agent message, approve a permission, start or
stop a run, or report workflow verification.

## Enable it deliberately

Build OMAR with the optional feature and enable the local API in the daemon
configuration. The per-run mode still defaults to `off`.

```toml
[decision_support]
enabled = true
# `jev-1.13.0` and a three-second timeout are the defaults.
model = "jev-1.13.0"
timeout_seconds = 3
```

Set `TYPESAFE_API_KEY` in the environment of `omar serve`. It is read only by
the provider client and is never returned, logged, or persisted.

Mission Control shows **Suggestions** only when the daemon advertises the
capability. Choose **Enable for this run**, select text from a captured review
output, and choose **Evaluate selection**. **Copy handoff note** copies text to
the clipboard; it never sends a message or changes a workflow.

## Modes and scope

`off` is the default and starts no capture or provider work. `shadow` captures
eligible review output for local inspection. `suggest` enables operator-requested
evaluations. Only review reactions writing to a `review` port are captured, and
the observer connects to the daemon-issued loopback diagram stream after run
admission. A stale or interrupted stream is labelled partial instead of being
silently treated as complete.

The service uses two bounded workers (depth 32), limits a run to 100 requests,
limits source data to 64 KiB and a selected request to 16 KiB, and does not
retry provider calls implicitly.

## Decision policy and records

Jev responses must resolve to `jev-1.13.0`, include the exact owner and
`sufficient_context` questions, and include complete probability distributions.
OMAR suggests a specific owner only when all three conditions are at least 0.90:
the selected specific owner probability, the sufficient-context probability,
and the response's explicit selection. `multiple`, `uncertain`, malformed, or
lower-confidence responses become `needs_review`.

Records live under `<omar_dir>/decisions/<run_id>/` with private permissions and
atomic file replacement. They contain source digests, filtered requests,
validated responses, decision status, coverage, and feedback. They never
contain the TypeSafe API key.

The local API is loopback-only:

- `GET /v1/assist/capabilities`
- `POST /v1/assist/runs/{id}/mode`
- `GET /v1/assist/runs/{id}/sources`
- `POST /v1/assist/runs/{id}/evaluations`
- `GET /v1/assist/runs/{id}/decisions`
- `POST /v1/assist/runs/{id}/decisions/{decision_id}/feedback`

Mutating routes reject non-loopback browser origins. A client talking to an
older daemon receives no Suggestions tab, preserving the existing product
surface during upgrades.
