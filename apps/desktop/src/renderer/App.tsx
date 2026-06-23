import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { alphaStateFixture, type AlphaState } from "../shared/alphaState";

type Tab = "overview" | "projects" | "hosted" | "pairing" | "sync" | "safety";

const tabs: { id: Tab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "projects", label: "Projects" },
  { id: "hosted", label: "Hosted" },
  { id: "pairing", label: "Pairing" },
  { id: "sync", label: "Live Sync" },
  { id: "safety", label: "Safety" }
];

export function App() {
  const [state, setState] = useState<AlphaState>(alphaStateFixture);
  const [activeTab, setActiveTab] = useState<Tab>("overview");

  const refreshState = () => {
    void window.bindhub?.getAlphaState().then(setState);
  };

  useEffect(() => {
    refreshState();
  }, []);

  const totals = useMemo(
    () => ({
      projects: state.projects.length,
      pending: state.projects.reduce((sum, project) => sum + project.pendingChanges, 0),
      conflicts: state.conflicts.filter((conflict) => conflict.status === "open").length,
      secrets: state.projects.reduce((sum, project) => sum + project.blockedSecrets, 0)
    }),
    [state]
  );

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">D</div>
          <div>
            <h1>Bindhub</h1>
            <p>Private alpha</p>
          </div>
        </div>
        <div className={`status-pill ${state.status}`}>{state.status}</div>
        <nav className="tabs" aria-label="Bindhub sections">
          {tabs.map((tab) => (
            <button
              key={tab.id}
              className={activeTab === tab.id ? "active" : ""}
              onClick={() => setActiveTab(tab.id)}
              type="button"
            >
              {tab.label}
            </button>
          ))}
        </nav>
      </aside>

      <section className="workspace">
        <header className="topline">
          <div>
            <p className="eyebrow">
              {state.source} / {state.remote.kind} / {state.liveSync.status}
            </p>
            <h2>Alpha Control Surface</h2>
            <p className="subtle">{state.statusLabel}</p>
          </div>
          <div className="top-actions">
            <button type="button" onClick={refreshState}>
              Refresh state
            </button>
          </div>
        </header>

        <div className="summary-strip" aria-label="Alpha summary">
          <Metric label="Projects" value={String(totals.projects)} />
          <Metric label="Pending" value={String(totals.pending)} />
          <Metric label="Conflicts" value={String(totals.conflicts)} />
          <Metric label="Secrets" value={String(totals.secrets)} />
          <Metric label="Session" value={state.hosted.sessionState} />
        </div>

        {activeTab === "overview" && (
          <Panel title="Configured Alpha Paths">
            <KeyValueGrid
              rows={[
                ["Local DB", state.local.dbPath],
                ["Blob cache", state.local.cacheRoot],
                ["Project root", state.local.projectRoot],
                ["Receiver target", state.local.targetPath],
                ["Local remote", state.local.remoteDir],
                ["Evidence path", state.local.evidenceDir]
              ]}
            />
            <CommandBlock label="Deterministic smoke test" command={state.commands.smokeTest} />
            <CommandBlock label="Desktop build" command={state.commands.desktopBuild} />
          </Panel>
        )}

        {activeTab === "projects" && (
          <Panel title="Watched Project State">
            <div className="table">
              <div className="table-row table-head">
                <span>Name</span>
                <span>Status</span>
                <span>Pending</span>
                <span>Remote</span>
              </div>
              {state.projects.map((project) => (
                <div className="table-row" key={project.id}>
                  <span>
                    <strong>{project.name}</strong>
                    <small>{project.path}</small>
                  </span>
                  <span className={`dot-label ${project.status}`}>{project.status}</span>
                  <span>{project.pendingChanges}</span>
                  <span>{project.remoteKind}</span>
                </div>
              ))}
            </div>
            <CommandBlock label="Live command" command={state.commands.liveSync} />
          </Panel>
        )}

        {activeTab === "hosted" && (
          <Panel title="Hosted API And Object Access">
            <KeyValueGrid
              rows={[
                ["Metadata API", state.hosted.api],
                ["Metadata DB", state.hosted.metadataDb],
                ["Account", state.hosted.metadataAccount],
                ["Project", state.hosted.metadataProject],
                ["Session env", state.hosted.sessionTokenEnv],
                ["Session state", state.hosted.sessionState],
                ["Remote bucket", state.remote.bucket],
                ["Remote prefix", state.remote.prefix],
                ["Object lease", state.remote.objectAccess.leaseId],
                ["Object grant", state.remote.objectAccess.grantStatus],
                ["Boundary", state.remote.objectAccess.sharedBucketBoundary],
                ["Capabilities", state.remote.objectAccess.capabilities]
              ]}
            />
            <CommandBlock label="Hosted login" command={state.hosted.commands.login} />
            <CommandBlock label="Hosted status" command={state.hosted.commands.status} />
            <CommandBlock label="Object access" command={state.hosted.commands.objectAccess} />
          </Panel>
        )}

        {activeTab === "pairing" && (
          <Panel title="Device Pairing Handoff">
            <KeyValueGrid
              rows={[
                ["Pairing state", state.pairing.status],
                ["Token env", state.pairing.tokenEnv],
                ["Join request env", state.pairing.joinRequestEnv],
                ["Completion env", state.pairing.completionEnv]
              ]}
            />
            <div className="table compact">
              {state.devices.map((device) => (
                <div className="table-row" key={device.id}>
                  <span>
                    <strong>{device.name}</strong>
                    <small>{device.id}</small>
                  </span>
                  <span>{device.role}</span>
                  <span>{device.trust}</span>
                  <span>{device.lastSeen}</span>
                </div>
              ))}
            </div>
            <CommandBlock label="Invite" command={state.pairing.commands.invite} />
            <CommandBlock label="Join" command={state.pairing.commands.join} />
            <CommandBlock label="Approve" command={state.pairing.commands.approveJoin} />
            <CommandBlock label="Complete" command={state.pairing.commands.complete} />
          </Panel>
        )}

        {activeTab === "sync" && (
          <Panel title="Live Sync Run State">
            <KeyValueGrid
              rows={[
                ["Status", state.liveSync.status],
                ["Mode", state.liveSync.mode],
                ["Once", String(state.liveSync.once)],
                ["Apply materialization", String(state.liveSync.apply)],
                ["Remote endpoint", state.remote.endpoint],
                ["Region", state.remote.region],
                ["Credentials", state.remote.credentials]
              ]}
            />
            <div className="note-list">
              {state.liveSync.notes.map((note) => (
                <p key={note}>{note}</p>
              ))}
            </div>
            <CommandBlock label="Run live sync" command={state.liveSync.command} />
            <CommandBlock label="Package CLI" command={state.commands.packageCli} />
            <CommandBlock label="Publish release" command={state.commands.publishCli} />
          </Panel>
        )}

        {activeTab === "safety" && (
          <Panel title="Safety And Redaction">
            {state.secrets.map((policy) => (
              <article className="record" key={`${policy.project}-${policy.path}`}>
                <div>
                  <strong>
                    {policy.project} / {policy.path}
                  </strong>
                  <p>{policy.note}</p>
                </div>
                <span className="action">{policy.action}</span>
                <code>{policy.envelopeRef ?? "raw material never printed"}</code>
              </article>
            ))}
            {state.conflicts.map((conflict) => (
              <article className="record" key={conflict.id}>
                <div>
                  <strong>{conflict.project}</strong>
                  <p>
                    {conflict.affectedPaths} path(s), {conflict.localSnapshot} vs{" "}
                    {conflict.incomingSnapshot}
                  </p>
                </div>
                <span className="action">{conflict.status}</span>
                <code>{conflict.command}</code>
              </article>
            ))}
          </Panel>
        )}
      </section>
    </main>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="metric">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function Panel({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="panel">
      <h3>{title}</h3>
      {children}
    </section>
  );
}

function KeyValueGrid({ rows }: { rows: [string, string][] }) {
  return (
    <div className="kv-grid">
      {rows.map(([label, value]) => (
        <div className="setting" key={label}>
          <span>{label}</span>
          <strong>{value}</strong>
        </div>
      ))}
    </div>
  );
}

function CommandBlock({ label, command }: { label: string; command: string }) {
  return (
    <div className="command-block">
      <span>{label}</span>
      <code>{command}</code>
    </div>
  );
}
