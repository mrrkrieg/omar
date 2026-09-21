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
# Jev is pinned to `jev-1.13.0`; three seconds is the default timeout.
timeout_seconds = 3
# Enroll exact reaction IDs in the review-owner profile.
review_owner_reactions = ["reaction::independent_reviewer"]
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
evaluations. The `review-owner-v1` profile captures only the exact reaction IDs
enrolled in `review_owner_reactions` when they write a `review` port. Each
source records its reaction invocation and diagram event sequence. The observer
connects to the daemon-issued loopback diagram stream after run admission. A
late, stale, interrupted, or gapped stream is labelled partial instead of being
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
atomic file replacement. They are reloaded after a daemon restart so request
IDs stay idempotent and a repeated selection is deduplicated. A request ID is
bound to its selected source and range, so it cannot be reused for other
content. They contain source digests, filtered requests, validated responses,
including the normalized probability distributions, decision status, coverage,
and feedback. Disabling a run waits for an already
dispatched bounded call, then invalidates queued work before it can become a
new suggestion. Records outside `retention_days` are unavailable through the
API, and the private store stops accepting new records at 16 MiB rather than
silently evicting evidence. They never contain the TypeSafe API key.

The local API is loopback-only:

- `GET /v1/assist/capabilities`
- `POST /v1/assist/runs/{id}/mode`
- `GET /v1/assist/runs/{id}/sources`
- `POST /v1/assist/runs/{id}/evaluations`
- `GET /v1/assist/runs/{id}/decisions`
- `POST /v1/assist/runs/{id}/decisions/{decision_id}/feedback`

The source and decision list routes return at most 50 records. Follow their
opaque `next_cursor` value with `?cursor=...` to retrieve the next page.

Mutating routes reject non-loopback browser origins. A client talking to an
older daemon receives no Suggestions tab, preserving the existing product
surface during upgrades.
