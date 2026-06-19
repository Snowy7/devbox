export type AlphaStatus = "ready" | "needs-config" | "command-only" | "blocked";

export type ConfigState = "configured" | "missing" | "not-used";

export type LocalPaths = {
  dbPath: string;
  cacheRoot: string;
  projectRoot: string;
  targetPath: string;
  remoteDir: string;
  evidenceDir: string;
};

export type HostedConfig = {
  api: string;
  metadataDb: string;
  metadataAccount: string;
  metadataProject: string;
  sessionTokenEnv: string;
  sessionState: ConfigState;
  authMode: "account-session" | "mock-dev-sqlite";
  commands: {
    login: string;
    status: string;
    objectAccess: string;
  };
};

export type RemoteConfig = {
  kind: "local-filesystem" | "s3-compatible";
  endpoint: string;
  bucket: string;
  region: string;
  prefix: string;
  credentials: string;
  objectAccess: {
    leaseId: string;
    capabilities: string;
    grantStatus: ConfigState;
    sharedBucketBoundary: string;
  };
};

export type PairingState = {
  status: "source-ready" | "receiver-pending" | "needs-token" | "command-only";
  tokenEnv: string;
  joinRequestEnv: string;
  completionEnv: string;
  commands: {
    invite: string;
    join: string;
    approveJoin: string;
    complete: string;
  };
};

export type LiveSyncState = {
  status: "ready-to-run" | "needs-config" | "command-only";
  mode: "push" | "pull" | "two-way";
  once: boolean;
  apply: boolean;
  command: string;
  notes: string[];
};

export type ProjectActivity = {
  id: string;
  name: string;
  path: string;
  status: AlphaStatus;
  lastSnapshot: string;
  pendingChanges: number;
  blockedSecrets: number;
  openConflicts: number;
  remoteKind: RemoteConfig["kind"];
};

export type ConflictSummary = {
  id: string;
  project: string;
  status: "open" | "resolved" | "dismissed";
  localSnapshot: string;
  incomingSnapshot: string;
  affectedPaths: number;
  command: string;
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
  role: "current" | "paired" | "pending";
  trust: "local" | "approved" | "pending-completion" | "rotation-pending";
  lastSeen: string;
};

export type AlphaState = {
  status: AlphaStatus;
  statusLabel: string;
  source: "environment" | "safe-placeholder";
  generatedAt: string;
  local: LocalPaths;
  hosted: HostedConfig;
  remote: RemoteConfig;
  pairing: PairingState;
  liveSync: LiveSyncState;
  projects: ProjectActivity[];
  conflicts: ConflictSummary[];
  secrets: SecretPolicy[];
  devices: DeviceSummary[];
  commands: {
    packageCli: string;
    publishCli: string;
    smokeTest: string;
    desktopBuild: string;
    liveSync: string;
  };
};

export function buildAlphaStateFromEnv(env: Record<string, string | undefined> = {}): AlphaState {
  const remoteKind = env.DEVBOX_REMOTE_KIND === "s3" ? "s3-compatible" : "local-filesystem";
  const local = buildLocalPaths(env);
  const hosted = buildHostedConfig(env);
  const remote = buildRemoteConfig(env, remoteKind, hosted);
  const pairing = buildPairingState(env, local);
  const liveSync = buildLiveSyncState(env, local, remoteKind);
  const source = hasAnyDevboxEnv(env) ? "environment" : "safe-placeholder";
  const status = deriveStatus(local, hosted, remote, liveSync);
  const projectName = projectNameFromPath(local.projectRoot);

  return {
    status,
    statusLabel: statusLabel(status),
    source,
    generatedAt: new Date().toISOString(),
    local,
    hosted,
    remote,
    pairing,
    liveSync,
    projects: [
      {
        id: hosted.metadataProject,
        name: projectName,
        path: local.projectRoot,
        status,
        lastSnapshot: "resolved by devbox-daemon sync",
        pendingChanges: 0,
        blockedSecrets: 0,
        openConflicts: 0,
        remoteKind
      }
    ],
    conflicts: [
      {
        id: "manual-conflict-template",
        project: projectName,
        status: "open",
        localSnapshot: "<local-snapshot-id>",
        incomingSnapshot: "<incoming-snapshot-id>",
        affectedPaths: 0,
        command:
          "devbox conflicts resolve --db " +
          shellValue(local.dbPath) +
          " <CONFLICT_ID> --manual-resolution keep-both --confirm-no-auto-apply"
      }
    ],
    secrets: [
      {
        project: projectName,
        path: ".env",
        action: "block",
        note: "Cloud/session variables stay in ignored env files and are not printed by the app."
      },
      {
        project: projectName,
        path: ".env.example",
        action: "template",
        note: "Committed examples contain placeholder names, never provider credentials."
      },
      {
        project: projectName,
        path: "local secret envelopes",
        action: "envelope",
        envelopeRef: "secret-envelope-ref:<opaque>",
        note: "Opaque references are acceptable; raw material stays local."
      }
    ],
    devices: [
      {
        id: "<current-device-id>",
        name: "Current machine",
        role: "current",
        trust: "local",
        lastSeen: "local state"
      },
      {
        id: "<paired-device-id>",
        name: "Paired laptop",
        role: pairing.status === "receiver-pending" ? "pending" : "paired",
        trust: pairing.status === "receiver-pending" ? "pending-completion" : "approved",
        lastSeen: "after devices complete"
      }
    ],
    commands: {
      packageCli: "scripts/package-cli.sh v0.1.0-alpha.1",
      publishCli: "scripts/publish-cli-release.sh v0.1.0-alpha.1",
      smokeTest: "scripts/alpha-two-device-smoke.sh",
      desktopBuild: "cd apps/desktop && npm run build",
      liveSync: liveSync.command
    }
  };
}

export const alphaStateFixture: AlphaState = buildAlphaStateFromEnv({});

function buildLocalPaths(env: Record<string, string | undefined>): LocalPaths {
  return {
    dbPath: env.DEVBOX_LIVE_DB ?? "./devbox.sqlite3",
    cacheRoot: env.DEVBOX_LIVE_CACHE ?? "./.devbox-cache",
    projectRoot: env.DEVBOX_LIVE_PROJECT_ROOT ?? "./project",
    targetPath: env.DEVBOX_LIVE_TARGET ?? "./receiver-project",
    remoteDir: env.DEVBOX_REMOTE_DIR ?? "./remote",
    evidenceDir: env.DEVBOX_ALPHA_EVIDENCE_DIR ?? "./.devbox-alpha-evidence"
  };
}

function buildHostedConfig(env: Record<string, string | undefined>): HostedConfig {
  const api = env.DEVBOX_METADATA_API ?? "http://127.0.0.1:8787";
  const metadataDb = env.DEVBOX_METADATA_DB ?? "./metadata-alpha.sqlite3";
  const metadataProject = env.DEVBOX_METADATA_PROJECT ?? "project-example";
  const sessionTokenEnv = env.DEVBOX_SESSION_TOKEN_ENV ?? "DEVBOX_SESSION_TOKEN";
  const sessionState = env[sessionTokenEnv] ? "configured" : "missing";

  return {
    api,
    metadataDb,
    metadataAccount: env.DEVBOX_METADATA_ACCOUNT ?? "<account-id-from-hosted-status>",
    metadataProject,
    sessionTokenEnv,
    sessionState,
    authMode: env.DEVBOX_METADATA_DB ? "mock-dev-sqlite" : "account-session",
    commands: {
      login:
        "devbox auth hosted-login --api " +
        shellValue(api) +
        " --email <tester-email> --invite-code-env DEVBOX_ALPHA_INVITE_CODE",
      status:
        "devbox auth hosted-status --api " +
        shellValue(api) +
        " --session-token-env " +
        shellValue(sessionTokenEnv),
      objectAccess:
        "devbox metadata object-access resolve --api " +
        shellValue(api) +
        " --session-token-env " +
        shellValue(sessionTokenEnv) +
        " --project " +
        shellValue(metadataProject) +
        " --lease " +
        shellValue(env.DEVBOX_OBJECT_ACCESS_LEASE ?? "lease-alpha")
    }
  };
}

function buildRemoteConfig(
  env: Record<string, string | undefined>,
  remoteKind: RemoteConfig["kind"],
  hosted: HostedConfig
): RemoteConfig {
  if (remoteKind === "local-filesystem") {
    return {
      kind: remoteKind,
      endpoint: "not used",
      bucket: "not used",
      region: "not used",
      prefix: "not used",
      credentials: "not-used",
      objectAccess: {
        leaseId: "not used",
        capabilities: "not used",
        grantStatus: "not-used",
        sharedBucketBoundary: "local remote directory only"
      }
    };
  }

  const accessKeyEnv = env.DEVBOX_R2_ACCESS_KEY_ENV ?? "DEVBOX_R2_ACCESS_KEY_ID";
  const secretKeyEnv = env.DEVBOX_R2_SECRET_KEY_ENV ?? "DEVBOX_R2_SECRET_ACCESS_KEY";
  const credentials =
    env[accessKeyEnv] && env[secretKeyEnv]
      ? "loaded from env names; values hidden"
      : "missing trusted-operator env names";

  return {
    kind: remoteKind,
    endpoint: env.DEVBOX_R2_ENDPOINT ?? "https://example-account-id.r2.cloudflarestorage.com",
    bucket: env.DEVBOX_R2_BUCKET ?? "devbox-alpha",
    region: env.DEVBOX_R2_REGION ?? "auto",
    prefix:
      env.DEVBOX_R2_PREFIX ??
      "accounts/" + hosted.metadataAccount + "/projects/" + hosted.metadataProject,
    credentials,
    objectAccess: {
      leaseId: env.DEVBOX_OBJECT_ACCESS_LEASE ?? "lease-alpha",
      capabilities: "read,write,list,head",
      grantStatus: hosted.sessionState,
      sharedBucketBoundary: "one bucket, per-account/project prefixes"
    }
  };
}

function buildPairingState(
  env: Record<string, string | undefined>,
  local: LocalPaths
): PairingState {
  const tokenEnv = env.DEVBOX_PAIRING_TOKEN_ENV ?? "DEVBOX_PAIRING_TOKEN";
  const joinRequestEnv = env.DEVBOX_PAIRING_JOIN_REQUEST_ENV ?? "DEVBOX_PAIRING_JOIN_REQUEST";
  const completionEnv = env.DEVBOX_PAIRING_COMPLETION_ENV ?? "DEVBOX_PAIRING_COMPLETION";
  const receiverDb = env.DEVBOX_RECEIVER_DB ?? "./receiver.sqlite3";
  const hasToken = Boolean(env[tokenEnv]);
  const hasJoinRequest = Boolean(env[joinRequestEnv]);
  const hasCompletion = Boolean(env[completionEnv]);
  const status = hasCompletion
    ? "receiver-pending"
    : hasJoinRequest
      ? "source-ready"
      : hasToken
        ? "needs-token"
        : "command-only";

  return {
    status,
    tokenEnv,
    joinRequestEnv,
    completionEnv,
    commands: {
      invite: "devbox devices invite --db " + shellValue(local.dbPath),
      join:
        "devbox devices join --db " +
        shellValue(receiverDb) +
        " --token-env " +
        shellValue(tokenEnv) +
        " --device-name <receiver-name>",
      approveJoin:
        "devbox devices approve-join --db " +
        shellValue(local.dbPath) +
        " --token-env " +
        shellValue(tokenEnv) +
        " --join-request-env " +
        shellValue(joinRequestEnv) +
        " --device-name <receiver-name>",
      complete:
        "devbox devices complete --db " +
        shellValue(receiverDb) +
        " --completion-env " +
        shellValue(completionEnv)
    }
  };
}

function buildLiveSyncState(
  env: Record<string, string | undefined>,
  local: LocalPaths,
  remoteKind: RemoteConfig["kind"]
): LiveSyncState {
  const mode = parseMode(env.DEVBOX_LIVE_MODE);
  const once = env.DEVBOX_LIVE_ONCE === "true";
  const apply = env.DEVBOX_LIVE_APPLY === "true";
  const missing = [
    ["DEVBOX_LIVE_DB", env.DEVBOX_LIVE_DB],
    ["DEVBOX_LIVE_CACHE", env.DEVBOX_LIVE_CACHE],
    ["DEVBOX_LIVE_PROJECT_ROOT", env.DEVBOX_LIVE_PROJECT_ROOT]
  ].filter(([, value]) => !value);
  const command = "scripts/devbox-live-sync-alpha.sh " + shellValue(env.DEVBOX_ALPHA_ENV_FILE ?? ".env.r2.local");
  const notes = [
    "No sync starts from the desktop until a local daemon bridge is wired.",
    remoteKind === "s3-compatible"
      ? "S3 mode validates the object-access grant and prefix, then still uses trusted-operator S3 env credentials for object transfer."
      : "Local remote mode uses the configured remote directory.",
    "Pending receiver identities fail closed until devices complete installs the local key envelope."
  ];

  return {
    status: missing.length === 0 ? "ready-to-run" : "needs-config",
    mode,
    once,
    apply,
    command,
    notes
  };
}

function deriveStatus(
  local: LocalPaths,
  hosted: HostedConfig,
  remote: RemoteConfig,
  liveSync: LiveSyncState
): AlphaStatus {
  void local;
  if (liveSync.status === "needs-config") {
    return "needs-config";
  }
  if (remote.kind === "s3-compatible" && hosted.sessionState === "missing") {
    return "needs-config";
  }
  if (remote.kind === "s3-compatible" && remote.credentials.startsWith("missing")) {
    return "command-only";
  }
  return "ready";
}

function statusLabel(status: AlphaStatus): string {
  switch (status) {
    case "ready":
      return "Ready to run configured alpha commands";
    case "needs-config":
      return "Missing required local alpha configuration";
    case "command-only":
      return "Command preview only; fill env before running";
    case "blocked":
      return "Blocked by safety preflight";
  }
}

function parseMode(value: string | undefined): LiveSyncState["mode"] {
  if (value === "pull" || value === "two-way") {
    return value;
  }
  return "push";
}

function hasAnyDevboxEnv(env: Record<string, string | undefined>): boolean {
  return Object.keys(env).some((key) => key.startsWith("DEVBOX_"));
}

function projectNameFromPath(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? "project";
}

function shellValue(value: string): string {
  if (/^<[^>]+>$/.test(value)) {
    return value;
  }
  if (/^[A-Za-z0-9_./:@=-]+$/.test(value)) {
    return value;
  }
  return "'" + value.replace(/'/g, "'\"'\"'") + "'";
}
