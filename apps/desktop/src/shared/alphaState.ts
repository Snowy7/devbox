export type AlphaStatus = "idle" | "syncing" | "paused" | "warning";

export type ProjectActivity = {
  id: string;
  name: string;
  path: string;
  status: AlphaStatus;
  lastSnapshot: string;
  pendingChanges: number;
  blockedSecrets: number;
  openConflicts: number;
};

export type ConflictSummary = {
  id: string;
  project: string;
  status: "open" | "resolved" | "dismissed";
  localSnapshot: string;
  incomingSnapshot: string;
  affectedPaths: number;
  manualOptions: string[];
};

export type SecretPolicy = {
  project: string;
  path: string;
  action: "block" | "template" | "envelope";
  envelopeRef?: string;
  note: string;
};

export type DeviceSummary = {
  id: string;
  name: string;
  role: "current" | "paired";
  trust: "local" | "approved" | "rotation-pending";
  lastSeen: string;
};

export type AlphaState = {
  status: AlphaStatus;
  accountMode: "local-alpha";
  syncMode: "no-network" | "local-remote";
  watcher: "running" | "paused";
  remote: {
    provider: "local-filesystem" | "s3-compatible";
    location: string;
    credentials: "not-used" | "env-redacted" | "managed-lease-redacted";
  };
  projects: ProjectActivity[];
  conflicts: ConflictSummary[];
  secrets: SecretPolicy[];
  devices: DeviceSummary[];
  commands: {
    init: string;
    snapshot: string;
    conflicts: string;
    secrets: string;
  };
};

export const alphaStateFixture: AlphaState = {
  status: "warning",
  accountMode: "local-alpha",
  syncMode: "no-network",
  watcher: "running",
  remote: {
    provider: "local-filesystem",
    location: "~/Library/Application Support/Devbox/alpha-remote",
    credentials: "not-used"
  },
  projects: [
    {
      id: "project-local-devbox",
      name: "devbox",
      path: "~/Code/devbox",
      status: "warning",
      lastSnapshot: "snapshot-b3-redacted-local",
      pendingChanges: 3,
      blockedSecrets: 1,
      openConflicts: 1
    },
    {
      id: "project-local-api",
      name: "api",
      path: "~/Code/api",
      status: "idle",
      lastSnapshot: "snapshot-b3-redacted-api",
      pendingChanges: 0,
      blockedSecrets: 0,
      openConflicts: 0
    }
  ],
  conflicts: [
    {
      id: "conflict-b3-redacted",
      project: "devbox",
      status: "open",
      localSnapshot: "snapshot-local-redacted",
      incomingSnapshot: "snapshot-laptop-redacted",
      affectedPaths: 4,
      manualOptions: ["keep-local", "keep-incoming", "keep-both", "exported"]
    }
  ],
  secrets: [
    {
      project: "devbox",
      path: ".env",
      action: "block",
      note: "Detected provider token shape; raw value is never printed."
    },
    {
      project: "devbox",
      path: ".env.example",
      action: "template",
      note: "Sync variable names and placeholders only."
    },
    {
      project: "api",
      path: "config/local.secrets.json",
      action: "envelope",
      envelopeRef: "secret-envelope-ref:personal/api-local",
      note: "Opaque encrypted envelope reference."
    }
  ],
  devices: [
    {
      id: "device-current-redacted",
      name: "Desktop",
      role: "current",
      trust: "local",
      lastSeen: "now"
    },
    {
      id: "device-laptop-redacted",
      name: "Laptop",
      role: "paired",
      trust: "approved",
      lastSeen: "2026-06-18T18:42:00Z"
    }
  ],
  commands: {
    init: "devbox init --db <DB_PATH> --device-name <NAME>",
    snapshot: "devbox snapshot --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>",
    conflicts:
      "devbox conflicts resolve --db <DB_PATH> <CONFLICT_ID> --manual-resolution keep-both --confirm-no-auto-apply",
    secrets:
      "devbox secrets policy add --db <DB_PATH> --project <PROJECT_ID> --path <REL_PATH> --action template"
  }
};
