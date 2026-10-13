# Jev topology backend

`team Review[reviewer : Jev]` evaluates a Choice through the TypeSafe API rather
than launching a coding-agent pane. Jev is registered by name/case like other
backends, but is topology-only and is not an executive-assistant backend.

Each prompt writes exactly one string-enum port, and optionally one `string`
port for JSON evidence. The enum can be imported with `type Decision from ...`.
The prompt describes the meaning of the options. The adapter currently uses
option labels as Choice criteria; explicit per-option criteria are future work.

```text
type Decision from "./schemas/decision.json"
team Review[reviewer : Jev]
{
    input request : string
    output out : Decision
    output evidence : string
    prompt reviewer(request) -> out, evidence
    "Choose continue if the request can proceed; stop if it cannot; defer if unsure."
}
main JevReview { review = Review() }
```

Schema: `{ "type": "string", "enum": ["continue", "stop", "defer"] }`.
Set `TYPESAFE_API_KEY` in the launching shell. The model defaults to `jev-1.13.0`;
set `OMAR_JEV_MODEL` to choose another model. Adapter settings survive both
foreground/background session creation. They are captured when the session
starts; changing a client shell does not reconfigure an existing session.
`curl` must be installed; its user curlrc is disabled for this API call.

The decision is a plain string checked against its port enum. Evidence is JSON
serialized into a string: `answers.decision` has `choice`, `probabilities`, and
`confidence`; `context` contains original trigger values. A Rust gate may parse
it with serde_json. `answers.sufficient_context` is a validated Noul assessment.
`routing.eligible` requires confidence, selected probability and sufficiency all
>= 0.95 and a non-defer outcome. The enum alone is a classification; connect an
explicit gate before downstream work. The shipped example does so. Evidence is not an imported object type.

Backend execution returns writes to the VM, which checks the effect contract and
port types before publishing. Native Choice membership/distribution validation
remains in the adapter. Registered topology-only backends get no tmux session.

## Failure policy

`OMAR_JEV_ON_ERROR=fail` is the default. Credentials/configuration errors,
transport errors, non-success HTTP status, timeout, invalid JSON/Choice output,
and oversized responses fail the invocation without publishing a decision.

Set `OMAR_JEV_ON_ERROR=defer` to opt into fallback. The invocation must have a
`defer` enum value AND an evidence string output, checked before the call. On
failure it writes defer with evidence like:

```json
{
  "model": "jev-1.13.0",
  "error": {
    "kind": "rate_limit",
    "message": "Jev rate limit exceeded",
    "http_status": 429
  },
  "context": {"review.request": "original input"}
}
```

Error kinds: configuration, authentication (401/403), rate_limit (429), service
(5xx), http (other non-success status), transport, timeout, invalid_response.
Messages never copy credentials, provider error bodies, or curl stderr. Neither
policy retries automatically. A model-selected defer is a normal Choice result;
a service failure has error evidence and no answers object.

Each HTTP call has a maximum 20-second budget, shortened to the invocation's
declared time limit. A zero budget makes no call. Parent-enforced deadlines terminate
the HTTP process group, including pipe-blocked helpers; curl also gets its own
timeout. Response data is capped at 1 MiB. There is no API-key argument or file:
headers and JSON body are supplied over stdin. Evidence is still a JSON string,
not an imported object schema.

The local experiment launchers opt into defer explicitly. Classification thresholds
remain entirely in the workflow's Rust reaction, independent of transport errors.

## Example

`examples/jev-review.omar` imports `schemas/jev-decision.json` and passes the enum
and evidence to a Rust gate. Its inclusive >=0.95 threshold checks all three scores. Confident continue/stop
choices route directly; all other choices reach the configured Codex reasoning
team. Only the resolved decision reaches `next`, which continues, repairs, or
blocks. Original context is retained on every path. Thresholds need evaluation
on your real tasks; fixture tests prove routing, not model calibration.

```sh
export TYPESAFE_API_KEY
export OMAR_JEV_ON_ERROR=defer  # optional; default is fail
omar run examples/jev-review.omar --input "review.request=Example request" --wait
```

Generated Rust reaction crates enable serde_json when a body references it. The
normal OMAR build caches its sources; reaction builds still use cargo --offline.

## Integrated builder and automatic gates

Native topology prompts and `jev(...)` gates share the same bounded transport.
Automatic gates keep the pinned model, 3-second/64-KiB advisory limits and
reasoning fallback from #287; native prompts retain their invocation deadline
and 1-MiB response cap. Both use an inclusive 0.95 cutoff, Choice confidence,
selected probability and Noul sufficiency. Native prompts also require the
response model to match the requested model.

The builder's `omar_propose_design` accepts a `files` map. Include each imported
JSON schema under its relative path with complete UTF-8 contents. The daemon
compiles that bundle before publishing the proposal; Mission Control shows the
imported files and sends the same bundle when checking or deploying it. Old
single-file proposals continue to work. Schema imports remain scalar string
enums; JSON evidence remains a plain string, so #248 needs no object-schema
extension. Paths must stay within the staged program directory.

For bounded artifact/owner judgments, prefer #287's `jev(...)` annotation on
the configured reasoning agent: it forwards the original string to exactly one
declared branch, calls Jev once, and reasons once on uncertainty or failure.
For arbitrary imported choice enums, use the native backend and an explicit
confidence gate as in `jev-review.omar`. Do not put a second Jev gate on its
reasoning fallback. Native service-error fallback requires
`OMAR_JEV_ON_ERROR=defer`; without it the invocation fails closed.
