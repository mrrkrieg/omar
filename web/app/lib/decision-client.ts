import type {
  DecisionCapabilities,
  DecisionMode,
  DecisionRecord,
  DecisionSource,
} from "./protocol";
import { normalizeRuntimeUrl } from "./runtime-client";

async function readError(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: unknown };
    if (typeof body.error === "string") return body.error;
  } catch { /* status fallback */ }
  return `Runtime returned HTTP ${response.status}.`;
}

export async function fetchDecisionCapabilities(serveUrl: string): Promise<DecisionCapabilities | null> {
  const response = await fetch(`${normalizeRuntimeUrl(serveUrl)}/v1/assist/capabilities`);
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(await readError(response));
  return (await response.json()) as DecisionCapabilities;
}

export async function setDecisionMode(serveUrl: string, runId: string, mode: DecisionMode): Promise<void> {
  const response = await fetch(`${normalizeRuntimeUrl(serveUrl)}/v1/assist/runs/${encodeURIComponent(runId)}/mode`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ mode }),
  });
  if (!response.ok) throw new Error(await readError(response));
}

export async function fetchDecisionSources(serveUrl: string, runId: string): Promise<DecisionSource[]> {
  const response = await fetch(`${normalizeRuntimeUrl(serveUrl)}/v1/assist/runs/${encodeURIComponent(runId)}/sources`);
  if (!response.ok) throw new Error(await readError(response));
  const body = (await response.json()) as { sources?: DecisionSource[] };
  return Array.isArray(body.sources) ? body.sources : [];
}

export async function fetchDecisions(serveUrl: string, runId: string): Promise<DecisionRecord[]> {
  const response = await fetch(`${normalizeRuntimeUrl(serveUrl)}/v1/assist/runs/${encodeURIComponent(runId)}/decisions`);
  if (!response.ok) throw new Error(await readError(response));
  const body = (await response.json()) as { decisions?: DecisionRecord[] };
  return Array.isArray(body.decisions) ? body.decisions : [];
}

export async function evaluateDecision(serveUrl: string, runId: string, request: {
  request_id: string; profile_id: string; source_id: string; source_sha256: string; selection_start: number; selection_end: number;
}): Promise<void> {
  const response = await fetch(`${normalizeRuntimeUrl(serveUrl)}/v1/assist/runs/${encodeURIComponent(runId)}/evaluations`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(request),
  });
  if (!response.ok) throw new Error(await readError(response));
}

export async function recordDecisionFeedback(serveUrl: string, runId: string, decisionId: string, outcome: "accepted" | "rejected" | "corrected"): Promise<void> {
  const response = await fetch(`${normalizeRuntimeUrl(serveUrl)}/v1/assist/runs/${encodeURIComponent(runId)}/decisions/${encodeURIComponent(decisionId)}/feedback`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ request_id: crypto.randomUUID(), outcome }),
  });
  if (!response.ok) throw new Error(await readError(response));
}
