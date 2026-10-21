"use client";

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { saveWorkflowName, useWorkflowNames } from "./lib/workflow-names";
import { WorkflowIcon } from "./workflow-icon";
import { WorkflowSortMenu, type WorkflowSort } from "./workflow-sort-menu";
import type { ConversationSummary } from "./lib/protocol";
import { fetchConversations, fetchRemovedConversations, setConversationRemoved, selectConversation } from "./lib/runtime-client";

const mobileQuery = "(max-width: 900px)";
function subscribeViewport(onChange: () => void) {
  const query = window.matchMedia(mobileQuery);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}
export function useHistoryDrawer() {
  return useSyncExternalStore(subscribeViewport, () => window.matchMedia(mobileQuery).matches, () => false);
}

export function OmarLogo() {
  // Static artwork needs no image loader or remote request.
  // eslint-disable-next-line @next/next/no-img-element
  return <img className="brand-mark" src="/omar-logo.png" alt="Omar" width={40} height={40} />;
}

export function SidebarIcon({ direction }: { direction: "open" | "close" }) {
  return (
    <svg className="sidebar-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true">
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M9 4v16" />
      {direction === "open" ? <path d="m12 9 3 3-3 3" /> : <path d="m14 9-3 3 3 3" />}
    </svg>
  );
}

export function ChatHistory({ serveUrl, activeId, mobile, revision, collapsed, onClose, onSelect, onSwitchingChange }: {
  serveUrl: string;
  activeId: string;
  mobile: boolean;
  revision: string;
  collapsed: boolean;
  onClose: () => void;
  onSelect: (conversation: ConversationSummary) => void;
  onSwitchingChange: (switching: boolean) => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [chats, setChats] = useState<ConversationSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [switching, setSwitching] = useState(false);
  const [error, setError] = useState("");
  const [actionError, setActionError] = useState("");
  const [query, setQuery] = useState("");
  const [showRemoved, setShowRemoved] = useState(false);
  const [removedChats, setRemovedChats] = useState<ConversationSummary[]>([]);
  const [lastRemoved, setLastRemoved] = useState<ConversationSummary | null>(null);
  const mutation = useRef(0);
  const selectionRef = useRef({activeId, onSelect});
  useEffect(() => { selectionRef.current = {activeId, onSelect}; }, [activeId, onSelect]);
  const [retry, setRetry] = useState(0);
  const [sort, setSort] = useState<WorkflowSort>("recent");
  const names = useWorkflowNames(serveUrl);
  const namesRef = useRef(names);
  useEffect(() => { namesRef.current = names; }, [names]);
  const [editing, setEditing] = useState<string | null>(null);
  const [editedName, setEditedName] = useState("");
  const cancelRename = useRef(false);
  const migratedNames = useRef(new Set<string>());
  const displayName = (chat: ConversationSummary) => chat.workflow_name ?? names[chat.id]?.name ?? chat.title;
  const updatedAt = (chat: ConversationSummary) => Math.max(chat.updated_at, names[chat.id]?.updatedAt ?? 0);
  async function saveName(id: string) {
    if (cancelRename.current) return;
    try { await saveWorkflowName(serveUrl, id, editedName); setEditing(null); setError(""); }
    catch (cause) { setError(cause instanceof Error ? cause.message : "Could not save the workflow name to this workspace."); }
  }

  // Only the narrow-screen drawer is modal. Desktop history stays beside the
  // conversation and never captures focus from the composer or topology.
  useEffect(() => {
    if (!mobile) return;
    const dialog = dialogRef.current;
    const opener = document.activeElement as HTMLElement | null;
    dialog?.showModal();
    return () => {
      dialog?.close();
      opener?.focus();
    };
  }, [mobile]);

  // The sidebar stays mounted: update its title, count, ordering and selection
  // after chat-stream changes, including selections made in another tab.
  useEffect(() => {
    const abort = new AbortController();
    const refresh = () => {
      const version = mutation.current;
      void Promise.all([fetchConversations(serveUrl, abort.signal), showRemoved ? fetchRemovedConversations(serveUrl, abort.signal) : Promise.resolve(null)]).then(([history, removed]) => {
        if (abort.signal.aborted || version !== mutation.current) return;
        if (removed) setRemovedChats(removed);
        const selection = selectionRef.current;
        if (history.removed_ids?.includes(selection.activeId)) {
          const next = history.conversations.find(chat => chat.id === history.active_id);
          if (next) selection.onSelect(next);
        }
        setChats(history.conversations);
        for (const chat of history.conversations) {
          const local = namesRef.current[chat.id];
          if (local && !chat.workflow_name && !migratedNames.current.has(chat.id)) {
            migratedNames.current.add(chat.id);
            void saveWorkflowName(serveUrl, chat.id, local.name).catch(() => migratedNames.current.delete(chat.id));
          }
        }
        setError("");
        setLoading(false);
      }).catch((cause) => {
        if (abort.signal.aborted) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        setLoading(false);
      });
    };
    const timer = setTimeout(refresh, 150);
    const poll = setInterval(refresh, 2000);
    return () => { clearTimeout(timer); clearInterval(poll); abort.abort(); };
  }, [serveUrl, revision, retry, showRemoved]);

  async function select(id?: string) {
    if (id === activeId) {
      if (mobile) onClose();
      return;
    }
    setSwitching(true);
    onSwitchingChange(true);
    setError("");
    try {
      const conversation = await selectConversation(serveUrl, id);
      setQuery("");
      setChats((current) => [conversation, ...current.filter((chat) => chat.id !== conversation.id)]);
      onSelect(conversation);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSwitching(false);
      onSwitchingChange(false);
    }
  }

  async function removeOrRestore(chat: ConversationSummary, removed: boolean) {
    mutation.current++;
    setSwitching(true);
    setActionError("");
    try {
      const history = await setConversationRemoved(serveUrl, chat.id, removed);
      setChats(history.conversations);
      setRemovedChats(current => removed ? [chat, ...current.filter(value => value.id !== chat.id)] : current.filter(value => value.id !== chat.id));
      setLastRemoved(removed ? chat : null);
      if (removed && chat.id === activeId) {
        const next = history.conversations.find(value => value.id === history.active_id)!;
        onSelect(next);
      }
      setRetry(current => current + 1);
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : String(cause));
    } finally { mutation.current++; setSwitching(false); }
  }

  const visible = (showRemoved ? removedChats : chats).filter((chat) => `${displayName(chat)} ${chat.run?.team ?? ""}`.toLowerCase().includes(query.toLowerCase())).sort((a, b) => {
    const byName = () => displayName(a).localeCompare(displayName(b), undefined, { sensitivity: "base", numeric: true });
    if (sort === "name") return byName() || updatedAt(b) - updatedAt(a);
    if (sort === "status") {
      const priority = workflowStatusPriority(a) - workflowStatusPriority(b);
      if (priority) return priority;
    }
    return updatedAt(b) - updatedAt(a) || byName();
  });
  // Folded: the divider's handle beside the conversation is the way back.
  if (collapsed) return null;
  const content = <>
    <header>
      <div className="brand-lockup"><OmarLogo /><strong>OMAR</strong></div>
    </header>
    <div className="sidebar-heading">
      <div><span className="eyebrow">WORKSPACE</span><h2 id="chat-history-title">Workflows</h2></div>
      <span className="workflow-count">{showRemoved ? removedChats.length : chats.length}</span>
    </div>
    <div className="history-actions">
      <button className="new-workflow" type="button" disabled={loading || switching} onClick={() => void select()}><span className="new-icon"><WorkflowIcon name="plus" size={15} /></span>New workflow</button>
      <label className="workflow-search"><WorkflowIcon name="search" size={15} /><input aria-label="Search workflows" placeholder="Search workflows…" value={query} onChange={(event) => setQuery(event.target.value)} /></label>
    </div>
    <div className="history-label"><span>{showRemoved ? "REMOVED" : sort === "recent" ? "RECENT" : sort === "name" ? "NAME" : "STATUS"}</span><div className="history-view-actions">
      <button type="button" className="history-trash-toggle" aria-label={showRemoved ? "Back to workflows" : "Removed workflows"} title={showRemoved ? "Back to workflows" : "Removed workflows"} aria-pressed={showRemoved} onClick={() => { setShowRemoved(value => !value); setQuery(""); setError(""); }}><WorkflowIcon name={showRemoved ? "arrow" : "trash"} size={16} /></button>
      {!showRemoved ? <WorkflowSortMenu value={sort} onChange={setSort} /> : null}
    </div></div>
    {lastRemoved ? <div className="history-undo" role="status"><span>Workflow removed.</span><button type="button" disabled={switching} onClick={() => void removeOrRestore(lastRemoved, false)}>Undo</button></div> : null}
    {error || actionError ? <div className="history-error"><p role="alert">{error || actionError}</p><button type="button" onClick={() => { setActionError(""); setRetry((current) => current + 1); }}>Retry</button></div> : null}
    <ul aria-label={showRemoved ? "Removed workflows" : "Saved chats"} aria-busy={loading || switching}>
      {visible.map((chat) => (
        <li key={chat.id} className="workflow-row">
          {showRemoved ? <div className="removed-workflow"><span className="workflow-title">{displayName(chat)}</span><small>{chat.message_count} messages · Saved workflow</small><button type="button" disabled={switching} onClick={() => void removeOrRestore(chat, false)}>Restore workflow</button></div> : <>
          <button className="workflow-select" type="button" disabled={switching} aria-current={chat.id === activeId ? "true" : undefined} aria-keyshortcuts="F2" onKeyDown={(event) => { if (event.key === "F2") { event.preventDefault(); cancelRename.current = false; setEditedName(displayName(chat)); setEditing(chat.id); } }} onClick={() => void select(chat.id)}>
            <span className="workflow-name"><span className="workflow-title" title={`${displayName(chat)} · Double-click to rename (saved automatically)`} onDoubleClick={(event) => { event.preventDefault(); event.stopPropagation(); cancelRename.current = false; setEditedName(displayName(chat)); setEditing(chat.id); }}>{displayName(chat)}</span>{chat.id === activeId ? <WorkflowIcon name="chevron" size={14} /> : null}</span>
            <span className="workflow-description">{chat.run?.team ?? (chat.message_count ? `${chat.message_count} messages in this workflow` : "Describe what you want to automate")}</span>
            <span className="workflow-meta"><small className={`workflow-status status-${workflowStatus(chat).toLowerCase()}`}><i />{workflowStatus(chat)}</small><time dateTime={new Date(updatedAt(chat)).toISOString()}>{new Date(updatedAt(chat)).toLocaleDateString(undefined, { month: "short", day: "numeric" })}</time>{chat.busy && chat.run ? <small>Thinking</small> : null}</span>
          </button>
          <button type="button" className="workflow-remove" aria-label="Remove workflow" title={removalReason(chat) || `Remove ${displayName(chat)} from history`} disabled={switching || !!removalReason(chat)} onClick={() => void removeOrRestore(chat, true)}><WorkflowIcon name="trash" size={16} /></button>
          {editing === chat.id ? <input className="workflow-rename" aria-label="Workflow name" maxLength={120} value={editedName} autoFocus onFocus={(event) => event.target.select()} onChange={(event) => setEditedName(event.target.value)} onBlur={() => void saveName(chat.id)} onKeyDown={(event) => {
            if (event.key === "Escape") { event.preventDefault(); cancelRename.current = true; setEditing(null); }
            if (event.key === "Enter") { event.preventDefault(); void saveName(chat.id); }
          }} /> : null}
          </>}
        </li>
      ))}
    </ul>
    {loading ? <p role="status">Loading workflows…</p> : visible.length === 0 && !error ? <p>{showRemoved ? "No removed workflows." : "No workflows found."}</p> : null}
  </>;
  return mobile ? (
    <dialog id="chat-history" ref={dialogRef} className="chat-history history-drawer" aria-label="Chat history" onCancel={onClose} onClick={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <div className="history-content">{content}</div>
    </dialog>
  ) : (
    <aside id="chat-history" className="chat-history" aria-label="Chat history">{content}</aside>
  );
}

/** The sidebar presents the runtime's actual state, including completed runs. */
function workflowStatus(chat: ConversationSummary): string {
  if (chat.run) return chat.run.status.charAt(0).toUpperCase() + chat.run.status.slice(1);
  return chat.busy ? "Thinking" : "Draft";
}

function workflowStatusPriority(chat: ConversationSummary): number {
  switch (workflowStatus(chat).toLowerCase()) {
    // Active work stays at the top, including startup and assistant activity.
    case "running": case "starting": case "thinking": return 0;
    case "failed": return 1;
    case "draft": return 2;
    case "paused": return 3;
    default: return 4;
  }
}

function removalReason(chat: ConversationSummary): string | null {
  if (chat.busy) return "Wait for the assistant to finish before removing this workflow.";
  if (chat.run && ["starting", "running", "stopping", "pausing", "paused"].includes(chat.run.status)) return "Stop this workflow before removing it from history.";
  return null;
}
