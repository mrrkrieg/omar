"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { scanSecrets, templates, type SecretFinding, type Template } from "./lib/templates";
import { fetchBackends } from "./lib/runtime-client";

const groups = [
  { id: "all", label: "All" },
  { id: "development", label: "Development" },
  { id: "security", label: "Security" },
  { id: "revenue", label: "Sales & marketing" },
  { id: "operations", label: "Operations & analysis" },
];

export function TemplateLibrary({ serveUrl, live, busy, onClose, onUse }: {
  serveUrl: string;
  live: boolean;
  busy: boolean;
  onClose: () => void;
  onUse: (template: Template, request: string, backend: string) => Promise<void>;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [group, setGroup] = useState("all");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<Template>(templates[0]);
  const [request, setRequest] = useState(templates[0].input);
  const [materials, setMaterials] = useState("");
  const [screening, setScreening] = useState("");
  const [findings, setFindings] = useState<SecretFinding[]>([]);
  const [scanned, setScanned] = useState(false);
  const [working, setWorking] = useState(false);
  const [error, setError] = useState("");
  const [backends, setBackends] = useState(["codex"]);
  const [backend, setBackend] = useState("codex");

  useEffect(() => {
    const node = dialog.current;
    const previous = document.activeElement as HTMLElement | null;
    node?.showModal();
    return () => { node?.close(); previous?.focus(); };
  }, []);

  useEffect(() => {
    if (!live) return;
    let active = true;
    void fetchBackends(serveUrl).then((info) => {
      if (!active) return;
      const supported = info.available.filter((name) => /^(agy|claude|codex|cursor|opencode|pi)$/i.test(name));
      if (supported.length) setBackends(supported);
      if (info.backend && supported.includes(info.backend)) setBackend(info.backend);
    }).catch(() => {});
    return () => { active = false; };
  }, [live, serveUrl]);

  const visible = useMemo(() => templates.filter((template) => {
    const text = `${template.title} ${template.outcome} ${template.audience}`.toLowerCase();
    return (group === "all" || template.group === group) && text.includes(query.toLowerCase());
  }), [group, query]);

  function select(template: Template) {
    setSelected(template);
    setRequest(template.input);
    setMaterials("");
    setScreening("");
    setFindings([]);
    setScanned(false);
    setError("");
  }

  async function prepareSelected() {
    if (working) return;
    setWorking(true);
    setError("");
    try {
      const combined = `${selected.field}: ${request.trim()}\n\nLocal source paths or supplied material:\n${materials.trim() || "Use the configured local workspace; no additional material supplied."}`;
      await onUse(selected, combined, backend);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setWorking(false);
    }
  }

  async function scanFiles(files: FileList | null) {
    if (!files) return;
    setError("");
    const next: SecretFinding[] = [];
    for (const file of Array.from(files).slice(0, 20)) {
      if (file.size > 1_000_000) {
        setError("Files must be under 1 MB each; the first 20 files are checked.");
        continue;
      }
      const matches = scanSecrets(await file.text());
      next.push(...matches.map((finding) => ({ ...finding, kind: `${file.name}: ${finding.kind}` })));
    }
    setFindings(next);
    setScanned(true);
  }

  return (
    <dialog ref={dialog} className="template-library" aria-labelledby="template-library-title" onCancel={onClose}>
      <header className="template-library-header">
        <div><span className="eyebrow">LOCAL WORKFLOWS</span><h2 id="template-library-title">Template library</h2><p>Choose a job, supply your own local material, and review the workflow before deployment.</p></div>
        <button type="button" className="secondary-button" onClick={onClose} aria-label="Close template library">Close</button>
      </header>
      <div className="template-library-body">
        <aside className="template-library-list" aria-label="Templates">
          <label className="template-search">Search templates<input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Job or outcome" /></label>
          <div className="template-groups" aria-label="Categories">{groups.map((item) => <button type="button" key={item.id} aria-pressed={group === item.id} onClick={() => setGroup(item.id)}>{item.label}</button>)}</div>
          <p className="template-count">{visible.length} of {templates.length} workflows</p>
          {visible.map((template) => <button className={`template-card${selected.id === template.id ? " selected" : ""}`} type="button" key={template.id} onClick={() => select(template)} aria-pressed={selected.id === template.id}>
            <span className="template-number">{String(template.number).padStart(2, "0")}</span><strong>{template.title}</strong><small>{template.outcome}</small>
          </button>)}
          {visible.length === 0 ? <p>No templates match.</p> : null}
        </aside>
        <section className="template-detail" aria-label="Selected template">
          <span className="eyebrow">{groups.find((item) => item.id === selected.group)?.label} · {selected.audience}</span>
          <h3>{selected.title}</h3>
          <p className="template-outcome">{selected.value}</p>
          <ol className="template-stages">{selected.stages.map((stage) => <li key={stage}>{stage}</li>)}</ol>
          <p><strong>What it does:</strong> {selected.scope}</p>
          <p><strong>Needs:</strong> {selected.prereq}</p>
          {selected.id === "secrets" ? (
            <div className="template-form">
              <p>Scan pasted text or selected files in this browser. Source text and token values are never sent to OMAR or an agent. Matches are redacted; this is a focused screening check, not a complete credential audit.</p>
              <label>Text to screen<textarea value={screening} onChange={(event) => { setScreening(event.target.value); setScanned(false); }} placeholder="Paste source text to screen locally" /></label>
              <label>Or local files<input type="file" multiple onChange={(event) => void scanFiles(event.target.files)} /></label>
              <button type="button" className="primary-button" onClick={() => { setFindings(scanSecrets(screening)); setScanned(true); }}>Scan text locally</button>
              {scanned ? <div className="template-findings" role="status"><h4>{findings.length} potential exposure{findings.length === 1 ? "" : "s"}</h4>{findings.map((finding, index) => <p key={index}>Line {finding.line} · {finding.kind} · {finding.redacted}</p>)}<p>Review matches locally. If a live credential is confirmed, follow your provider’s rotation procedure.</p></div> : null}
            </div>
          ) : (
            <div className="template-form">
              <label>{selected.field}<textarea value={request} onChange={(event) => setRequest(event.target.value)} /></label>
              <label>Local paths or source material<textarea value={materials} onChange={(event) => setMaterials(event.target.value)} placeholder="Give paths in the configured workspace, or paste the source material and rules needed for this job." /></label>
              <label>Agent backend<select value={backend} onChange={(event) => setBackend(event.target.value)}>{backends.map((name) => <option key={name} value={name}>{name}</option>)}</select></label>
              <p>Agents can inspect the runtime’s configured local workspace. Pasted material is sent to your configured agent backend only after you confirm deployment. The workflow produces a draft and evidence references; check results and calculations yourself before acting on them.</p>
              <button type="button" className="primary-button" disabled={!live || busy || working || !request.trim()} onClick={() => void prepareSelected()}>{working ? "Checking workflow…" : "Prepare workflow"}</button>
              {!live ? <p role="status">Connect to a live OMAR runtime to prepare this workflow.</p> : null}
              {busy ? <p role="status">Finish or stop the current run before preparing another workflow.</p> : null}
            </div>
          )}
          <details className="template-example"><summary>See a precomputed example</summary><pre>{selected.example.summary}</pre><pre>{selected.example.evidence}</pre></details>
          {error ? <p className="connection-error" role="alert">{error}</p> : null}
        </section>
      </div>
    </dialog>
  );
}
