import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { alphaStateFixture, type AlphaState } from "../shared/alphaState";

type Tab = "projects" | "activity" | "conflicts" | "devices" | "secrets" | "settings";

const tabs: { id: Tab; label: string }[] = [
  { id: "projects", label: "Projects" },
  { id: "activity", label: "Activity" },
  { id: "conflicts", label: "Conflicts" },
  { id: "devices", label: "Devices" },
  { id: "secrets", label: "Secrets" },
  { id: "settings", label: "Settings" }
];

export function App() {
  const [state, setState] = useState<AlphaState>(alphaStateFixture);
  const [activeTab, setActiveTab] = useState<Tab>("projects");

  useEffect(() => {
    void window.devbox?.getAlphaState().then(setState);
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
            <h1>Devbox</h1>
            <p>Private alpha</p>
          </div>
        </div>
        <div className={`status-pill ${state.status}`}>{state.status}</div>
        <nav className="tabs" aria-label="Devbox sections">
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
            <p className="eyebrow">{state.accountMode} / {state.syncMode}</p>
            <h2>Local control surface</h2>
          </div>
          <div className="summary-strip" aria-label="Alpha summary">
            <Metric label="Projects" value={totals.projects} />
            <Metric label="Pending" value={totals.pending} />
            <Metric label="Conflicts" value={totals.conflicts} />
            <Metric label="Secrets" value={totals.secrets} />
          </div>
        </header>

        {activeTab === "projects" && (
          <Panel title="Watched Projects">
            <div className="table">
              <div className="table-row table-head">
                <span>Name</span>
                <span>Status</span>
                <span>Pending</span>
                <span>Safety</span>
              </div>
              {state.projects.map((project) => (
                <div className="table-row" key={project.id}>
                  <span>
                    <strong>{project.name}</strong>
                    <small>{project.path}</small>
                  </span>
                  <span className={`dot-label ${project.status}`}>{project.status}</span>
                  <span>{project.pendingChanges}</span>
                  <span>{project.blockedSecrets} blocked · {project.openConflicts} conflicts</span>
                </div>
              ))}
            </div>
          </Panel>
        )}

        {activeTab === "activity" && (
          <Panel title="Sync Activity">
            <div className="activity-list">
              {state.projects.map((project) => (
                <div className="activity" key={project.id}>
                  <span className={`rail ${project.status}`} />
                  <div>
                    <strong>{project.name}</strong>
                  <p>{project.pendingChanges} pending change(s), last snapshot {project.lastSnapshot}</p>
                  </div>
                </div>
              ))}
            </div>
          </Panel>
        )}

        {activeTab === "conflicts" && (
          <Panel title="Manual Conflict Resolution">
            {state.conflicts.map((conflict) => (
              <article className="record" key={conflict.id}>
                <div>
                  <strong>{conflict.project}</strong>
                  <p>{conflict.affectedPaths} path(s), {conflict.localSnapshot} vs {conflict.incomingSnapshot}</p>
                </div>
                <code>{state.commands.conflicts}</code>
              </article>
            ))}
          </Panel>
        )}

        {activeTab === "devices" && (
          <Panel title="Devices And Pairing">
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
          </Panel>
        )}

        {activeTab === "secrets" && (
          <Panel title="Secret Safety Policy">
            {state.secrets.map((policy) => (
              <article className="record" key={`${policy.project}-${policy.path}`}>
                <div>
                  <strong>{policy.project} / {policy.path}</strong>
                  <p>{policy.note}</p>
                </div>
                <span className="action">{policy.action}</span>
                <code>{policy.envelopeRef ?? "raw material never printed"}</code>
              </article>
            ))}
          </Panel>
        )}

        {activeTab === "settings" && (
          <Panel title="Alpha Settings">
            <div className="settings-grid">
              <Setting label="Watcher" value={state.watcher} />
              <Setting label="Remote provider" value={state.remote.provider} />
              <Setting label="Remote location" value={state.remote.location} />
              <Setting label="Credentials" value={state.remote.credentials} />
            </div>
            <div className="command-stack">
              <code>{state.commands.init}</code>
              <code>{state.commands.snapshot}</code>
              <code>{state.commands.secrets}</code>
            </div>
          </Panel>
        )}
      </section>
    </main>
  );
}

function Metric({ label, value }: { label: string; value: number }) {
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

function Setting({ label, value }: { label: string; value: string }) {
  return (
    <div className="setting">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}
