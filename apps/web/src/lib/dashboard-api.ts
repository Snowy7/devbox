import type { UserInfo } from "@workos/authkit-tanstack-react-start"

import { identityFromWorkOsAuth, type BindhubWebIdentity } from "@/lib/identity"

export type DashboardDataMode =
  | "hosted-workos"
  | "local-dev-api"
  | "local-dev-fixtures"

export type DashboardDataSource = {
  mode: DashboardDataMode
  label: string
  baseUrl: string | null
}

export type SharedFolderSummary = {
  id: string
  displayName: string
  description: string
  localPath: string
  role: "owner" | "editor" | "viewer"
  accountId: string
  visibility: "private" | "public" | "team"
  syncStatus: "synced" | "syncing" | "attention"
  hydrationState: "remote-only" | "partial" | "fully-local"
  lastCheckpoint: string
  updatedAt: string
  sizeLabel: string
  fileCount: number
  machineCount: number
  entries: SharedFolderEntry[]
  revisions: FolderRevision[]
  settings: SharedFolderSettings
  recentActivity: FolderActivity[]
  readme: string
}

export type SharedFolderEntry = {
  path: string
  name: string
  kind: "directory" | "file"
  parentPath: string | null
  sizeLabel: string
  updatedAt: string
  hydrationState: SharedFolderSummary["hydrationState"]
  language?: string
  summary: string
  content?: string
}

export type FolderRevision = {
  id: string
  label: string
  message: string
  kind: "auto" | "checkpoint"
  createdAt: string
  author: string
  changedFiles: number
  pinned: boolean
}

export type SharedFolderSettings = {
  syncState: "live" | "paused"
  cachePolicy: "online-first" | "offline-pinned" | "low-disk" | "agent-sandbox"
  includeGitMetadata: boolean
  allowAgentSandboxes: boolean
  protectSecrets: boolean
  visibility: SharedFolderSummary["visibility"]
}

export type FolderActivity = {
  id: string
  title: string
  detail: string
  timestamp: string
}

export type MachineSummary = {
  id: string
  displayName: string
  accountId: string
  trustState: "trusted" | "pending"
  lastSeen: string
}

export type DashboardOverview = {
  folderCount: number
  machineCount: number
  trustedMachineCount: number
}

export type DashboardData = {
  identity: BindhubWebIdentity
  source: DashboardDataSource
  overview: DashboardOverview
  folders: SharedFolderSummary[]
  machines: MachineSummary[]
}

type SharedFolderWire = {
  id: string
  account_id: string
  role: SharedFolderSummary["role"]
  display_name: string
}

type DeviceWire = {
  id: string
  account_id: string
  display_name: string
}

type SharedFolderTreeWire = {
  revision_id: string | null
  file_count: number
  entries: SharedFolderTreeEntryWire[]
  revisions: SharedFolderRevisionWire[]
}

type SharedFolderTreeEntryWire = {
  path: string
  name: string
  kind: "directory" | "file" | "symlink" | "unsupported"
  parent_path: string | null
  size_bytes: number | null
  updated_at: string
  object_id: string | null
}

type SharedFolderRevisionWire = {
  id: string
  parent_id: string | null
  boundary: string
  created_at: string
  changed_files: number
}

type BindhubSessionWire = {
  account_id: string
  session_id: string
  session_token: string
  device_id: string
}

type BindhubApiSession = {
  sessionToken: string
  deviceId: string
}

type CliDeviceFlowApproval = {
  accountId: string
  sessionId: string
  deviceId: string
}

export async function loadDashboardData(
  auth: UserInfo
): Promise<DashboardData> {
  const identity = identityFromWorkOsAuth(auth)
  const source = readDashboardDataSource(process.env)

  if (source.mode === "hosted-workos") {
    const session = await exchangeWorkOsSession(source, auth)

    return dataFromApi(identity, source, {
      Authorization: `Bearer ${session.sessionToken}`,
      "x-bindhub-device-id": session.deviceId,
    })
  }

  if (source.mode === "local-dev-api") {
    const token = requiredEnv("BINDHUB_LOCAL_API_SESSION_TOKEN")
    const deviceId = requiredEnv("BINDHUB_LOCAL_API_DEVICE_ID")

    return dataFromApi(identity, source, {
      Authorization: `Bearer ${token}`,
      "x-bindhub-device-id": deviceId,
    })
  }

  return fixtureDashboardData(identity, source)
}

export async function approveCliDeviceLogin(
  userCode: string,
  auth: UserInfo
): Promise<CliDeviceFlowApproval> {
  return approveCliDeviceLoginWithVerifiedIdentity(userCode, {
    userId: auth.user.id,
    sessionId: auth.sessionId,
    organizationId: auth.organizationId ?? null,
  })
}

export async function approveLocalDevCliDeviceLogin(
  userCode: string
): Promise<CliDeviceFlowApproval> {
  if (process.env.BINDHUB_LOCAL_DEV_CLI_AUTH !== "1") {
    throw new Error("local-dev CLI auth is not enabled")
  }

  const localIdentity =
    process.env.BINDHUB_LOCAL_DEV_AUTH_EMAIL?.trim() || "local-dev@example.test"

  return approveCliDeviceLoginWithVerifiedIdentity(userCode, {
    userId: `local-dev-${localIdentity}`,
    sessionId: `local-dev-cli-${userCode.toLowerCase()}`,
    organizationId: null,
  })
}

async function approveCliDeviceLoginWithVerifiedIdentity(
  userCode: string,
  identity: {
    userId: string
    sessionId: string
    organizationId: string | null
  }
): Promise<CliDeviceFlowApproval> {
  const baseUrl = readCliAuthApiBaseUrl(process.env)
  const serviceToken = requiredEnv("BINDHUB_HOSTED_API_SERVICE_TOKEN")
  const response = await fetch(
    `${baseUrl}/v1/auth/cli-device-flow/${encodeURIComponent(userCode)}/approve`,
    {
      method: "POST",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
        "x-bindhub-api-service-token": serviceToken,
      },
      body: JSON.stringify({
        user_id: identity.userId,
        session_id: identity.sessionId,
        organization_id: identity.organizationId,
      }),
    }
  )

  if (!response.ok) {
    throw new Error(`Bindhub CLI auth approval failed: ${response.status}`)
  }

  const session = (await response.json()) as BindhubSessionWire

  return {
    accountId: session.account_id,
    sessionId: session.session_id,
    deviceId: session.device_id,
  }
}

function readCliAuthApiBaseUrl(env: NodeJS.ProcessEnv): string {
  return requireUrl(
    env.BINDHUB_HOSTED_API_URL?.trim() || env.BINDHUB_LOCAL_API_URL?.trim(),
    "BINDHUB_HOSTED_API_URL or BINDHUB_LOCAL_API_URL"
  )
}

export function readDashboardDataSource(
  env: NodeJS.ProcessEnv
): DashboardDataSource {
  const mode = normalizeMode(env.BINDHUB_DASHBOARD_DATA_MODE)
  const hostedBaseUrl = env.BINDHUB_HOSTED_API_URL?.trim()
  const localBaseUrl = env.BINDHUB_LOCAL_API_URL?.trim()

  if (mode === "hosted-workos") {
    return {
      mode,
      label: "Hosted API with WorkOS bearer auth",
      baseUrl: requireUrl(hostedBaseUrl, "BINDHUB_HOSTED_API_URL"),
    }
  }

  if (mode === "local-dev-api") {
    return {
      mode,
      label: "Local dev API session",
      baseUrl: requireUrl(localBaseUrl, "BINDHUB_LOCAL_API_URL"),
    }
  }

  return {
    mode: "local-dev-fixtures",
    label: "Local dev typed fixtures",
    baseUrl: null,
  }
}

async function exchangeWorkOsSession(
  source: DashboardDataSource,
  auth: UserInfo
): Promise<BindhubApiSession> {
  const baseUrl = source.baseUrl
  const serviceToken = requiredEnv("BINDHUB_HOSTED_API_SERVICE_TOKEN")

  if (!baseUrl) {
    throw new Error("dashboard API base URL is not configured")
  }

  const response = await fetch(`${baseUrl}/v1/auth/workos-session`, {
    method: "POST",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
      "x-bindhub-api-service-token": serviceToken,
    },
    body: JSON.stringify({
      user_id: auth.user.id,
      session_id: auth.sessionId,
      organization_id: auth.organizationId ?? null,
      device_id: `web-${auth.sessionId}`,
      device_display_name: "Bindhub web session",
    }),
  })

  if (!response.ok) {
    throw new Error(`Bindhub API session exchange failed: ${response.status}`)
  }

  const session = (await response.json()) as BindhubSessionWire

  return {
    sessionToken: session.session_token,
    deviceId: session.device_id,
  }
}

async function dataFromApi(
  identity: BindhubWebIdentity,
  source: DashboardDataSource,
  headers: HeadersInit
): Promise<DashboardData> {
  const baseUrl = source.baseUrl

  if (!baseUrl) {
    throw new Error("dashboard API base URL is not configured")
  }

  const [folders, machines] = await Promise.all([
    fetchJson<SharedFolderWire[]>(`${baseUrl}/v1/shared-folders`, headers),
    fetchJson<DeviceWire[]>(`${baseUrl}/v1/devices`, headers),
  ])
  const trees = await Promise.all(
    folders.map((folder) =>
      fetchJson<SharedFolderTreeWire>(
        `${baseUrl}/v1/loom/shared-folders/${encodeURIComponent(folder.id)}/tree`,
        headers
      ).catch(() => emptyHostedTree())
    )
  )

  return {
    identity,
    source,
    overview: overview(folders, machines),
    folders: folders.map((folder, index) => ({
      id: folder.id,
      displayName: folder.display_name,
      description: "Hosted shared folder",
      localPath: "~/Code",
      role: folder.role,
      accountId: folder.account_id,
      visibility: folder.role === "viewer" ? "team" : "private",
      syncStatus: "synced",
      hydrationState: "fully-local",
      lastCheckpoint: trees[index].revision_id ?? "No synced revision yet",
      updatedAt: latestTreeTimestamp(trees[index]) ?? "Known to Bindhub API",
      sizeLabel: "Remote metadata",
      fileCount: trees[index].file_count,
      machineCount: machines.length,
      entries: hostedEntriesFromTree(trees[index]),
      revisions: hostedRevisionsFromTree(trees[index]),
      settings: hostedPlaceholderSettings(folder.role === "viewer" ? "team" : "private"),
      recentActivity: [
        {
          id: `${folder.id}-api-sync`,
          title: "Synced from hosted API",
          detail:
            trees[index].revision_id
              ? `${trees[index].file_count} files are visible from the latest Loom revision.`
              : "Folder membership is live. No Loom revision has synced yet.",
          timestamp: "Now",
        },
      ],
      readme: hostedReadmeFromTree(folder.display_name, trees[index]),
    })),
    machines: machines.map((machine) => ({
      id: machine.id,
      displayName: machine.display_name,
      accountId: machine.account_id,
      trustState: "trusted",
      lastSeen: "Known to Bindhub API",
    })),
  }
}

async function fetchJson<T>(url: string, headers: HeadersInit): Promise<T> {
  const response = await fetch(url, {
    headers: {
      Accept: "application/json",
      ...headers,
    },
  })

  if (!response.ok) {
    throw new Error(`Bindhub API request failed: ${response.status}`)
  }

  return response.json() as Promise<T>
}

function fixtureDashboardData(
  identity: BindhubWebIdentity,
  source: DashboardDataSource
): DashboardData {
  const folders: SharedFolderSummary[] = [
    {
      id: "folder-primary-code",
      displayName: "Primary code folder",
      description: "Main developer workspace synced across desktop and laptop.",
      localPath: "~/Code",
      role: "owner",
      accountId: identity.account.accountId,
      visibility: "private",
      syncStatus: "synced",
      hydrationState: "fully-local",
      lastCheckpoint: "Ready on this machine",
      updatedAt: "2 minutes ago",
      sizeLabel: "1.8 GB",
      fileCount: 684,
      machineCount: 3,
      entries: [
        {
          path: "apps",
          name: "apps",
          kind: "directory",
          parentPath: null,
          sizeLabel: "142 MB",
          updatedAt: "2 minutes ago",
          hydrationState: "fully-local",
          summary: "Dashboard, site, and desktop surfaces.",
        },
        {
          path: "apps/web",
          name: "web",
          kind: "directory",
          parentPath: "apps",
          sizeLabel: "86 MB",
          updatedAt: "2 minutes ago",
          hydrationState: "fully-local",
          summary: "TanStack product dashboard and folder browser.",
        },
        {
          path: "apps/site",
          name: "site",
          kind: "directory",
          parentPath: "apps",
          sizeLabel: "31 MB",
          updatedAt: "26 minutes ago",
          hydrationState: "fully-local",
          summary: "Public site and docs surface.",
        },
        {
          path: "apps/web/src/routes/folders.$folderId.tsx",
          name: "folders.$folderId.tsx",
          kind: "file",
          parentPath: "apps/web",
          sizeLabel: "18 KB",
          updatedAt: "Now",
          hydrationState: "fully-local",
          language: "TSX",
          summary: "Shared folder detail route.",
          content:
            "export const Route = createFileRoute('/folders/$folderId')({\n  loader: async ({ location }) => loadAuthenticatedDashboard(location.pathname),\n  component: FolderDetailPage,\n})\n\nfunction FolderDetailPage() {\n  return <SharedFolderDetail />\n}\n",
        },
        {
          path: "Bindhub",
          name: "Bindhub",
          kind: "directory",
          parentPath: null,
          sizeLabel: "96 MB",
          updatedAt: "11 minutes ago",
          hydrationState: "fully-local",
          summary: "Hosted API and product CLI crates.",
        },
        {
          path: "loom",
          name: "loom",
          kind: "directory",
          parentPath: null,
          sizeLabel: "212 MB",
          updatedAt: "18 minutes ago",
          hydrationState: "partial",
          summary: "Folder revision engine and storage primitives.",
        },
        {
          path: "loom/store",
          name: "store",
          kind: "directory",
          parentPath: "loom",
          sizeLabel: "88 MB",
          updatedAt: "18 minutes ago",
          hydrationState: "partial",
          summary: "Object metadata, cache entries, and pack code.",
        },
        {
          path: "loom/store/cache.rs",
          name: "cache.rs",
          kind: "file",
          parentPath: "loom/store",
          sizeLabel: "41 KB",
          updatedAt: "18 minutes ago",
          hydrationState: "partial",
          language: "Rust",
          summary: "Cache metadata and remote byte availability.",
          content:
            "pub enum HydrationState {\n    RemoteOnly,\n    Partial,\n    FullyLocal,\n}\n\npub struct CacheEntry {\n    pub object_id: ObjectId,\n    pub hydration: HydrationState,\n    pub verified_remote: bool,\n}\n",
        },
        {
          path: "README.md",
          name: "README.md",
          kind: "file",
          parentPath: null,
          sizeLabel: "18 KB",
          updatedAt: "2 minutes ago",
          hydrationState: "fully-local",
          language: "Markdown",
          summary: "Product overview and local development notes.",
          content:
            "# Bindhub\n\nbindhub keeps developer folders continuous across machines. A shared folder can contain many repos, nested apps, secrets, dependencies, and agent sandboxes.\n\nLoom is the source-control and sync engine underneath Bindhub.",
        },
        {
          path: "Cargo.toml",
          name: "Cargo.toml",
          kind: "file",
          parentPath: null,
          sizeLabel: "1 KB",
          updatedAt: "1 hour ago",
          hydrationState: "fully-local",
          language: "TOML",
          summary: "Rust workspace manifest.",
          content:
            "[workspace]\nmembers = [\n  \"loom/*\",\n  \"bindhub/*\",\n]\nresolver = \"2\"\n",
        },
        {
          path: "pnpm-workspace.yaml",
          name: "pnpm-workspace.yaml",
          kind: "file",
          parentPath: null,
          sizeLabel: "126 B",
          updatedAt: "22 minutes ago",
          hydrationState: "fully-local",
          language: "YAML",
          summary: "Web workspace package map.",
          content:
            "packages:\n  - \"apps/*\"\n  - \"packages/*\"\n",
        },
      ],
      revisions: [
        {
          id: "rev-primary-1842",
          label: "checkpoint: web auth foundation",
          message: "Custom WorkOS UI and dashboard shell are ready.",
          kind: "checkpoint",
          createdAt: "22 minutes ago",
          author: "snowy",
          changedFiles: 64,
          pinned: true,
        },
        {
          id: "rev-primary-1841",
          label: "auto: folder browser edits",
          message: "Captured file browser route and fixture tree expansion.",
          kind: "auto",
          createdAt: "Now",
          author: "Bindhub desktop",
          changedFiles: 9,
          pinned: false,
        },
        {
          id: "rev-primary-1835",
          label: "checkpoint: cache policy",
          message: "Sparse hydration and cache warmup policy merged.",
          kind: "checkpoint",
          createdAt: "Yesterday",
          author: "loom",
          changedFiles: 31,
          pinned: true,
        },
      ],
      settings: {
        syncState: "live",
        cachePolicy: "online-first",
        includeGitMetadata: false,
        allowAgentSandboxes: true,
        protectSecrets: true,
        visibility: "private",
      },
      recentActivity: [
        {
          id: "activity-web-move",
          title: "Moved web apps into root workspace",
          detail: "apps/web, apps/site, and packages/ui are ready under pnpm.",
          timestamp: "22 minutes ago",
        },
        {
          id: "activity-api-env",
          title: "Local API env matched to web auth",
          detail: "Service-token approval is wired for local browser login.",
          timestamp: "35 minutes ago",
        },
        {
          id: "activity-home-ui",
          title: "Home UI pass in progress",
          detail: "Folder browsing now has a product-level route.",
          timestamp: "Now",
        },
      ],
      readme:
        "Primary code folder keeps the active Bindhub workspace continuous across machines. It can contain many repos, nested apps, generated dependencies, secrets, and agent sandboxes. Bindhub treats it as a shared folder first and lets Loom handle folder revisions underneath.",
    },
    {
      id: "folder-agent-sandbox",
      displayName: "Agent sandbox",
      description: "Isolated parallel work area for agent edits and review loops.",
      localPath: "~/Code/.bindhub/agent-sandbox",
      role: "editor",
      accountId: identity.account.accountId,
      visibility: "team",
      syncStatus: "syncing",
      hydrationState: "partial",
      lastCheckpoint: "Waiting for next checkpoint",
      updatedAt: "12 minutes ago",
      sizeLabel: "420 MB",
      fileCount: 128,
      machineCount: 2,
      entries: [
        {
          path: "worktrees",
          name: "worktrees",
          kind: "directory",
          parentPath: null,
          sizeLabel: "311 MB",
          updatedAt: "12 minutes ago",
          hydrationState: "partial",
          summary: "Parallel workspaces for implementation and review.",
        },
        {
          path: "worktrees/pr-review",
          name: "pr-review",
          kind: "directory",
          parentPath: "worktrees",
          sizeLabel: "184 MB",
          updatedAt: "12 minutes ago",
          hydrationState: "partial",
          summary: "Review workspace with source available on demand.",
        },
        {
          path: "worktrees/pr-review/notes.md",
          name: "notes.md",
          kind: "file",
          parentPath: "worktrees/pr-review",
          sizeLabel: "5 KB",
          updatedAt: "12 minutes ago",
          hydrationState: "fully-local",
          language: "Markdown",
          summary: "Reviewer notes and requested changes.",
          content:
            "# Review notes\n\n- Verify sparse folders do not become deletions.\n- Keep object byte availability separate from folder revisions.\n- Route review comments back to the implementer thread.",
        },
        {
          path: "review-notes.md",
          name: "review-notes.md",
          kind: "file",
          parentPath: null,
          sizeLabel: "9 KB",
          updatedAt: "18 minutes ago",
          hydrationState: "fully-local",
          language: "Markdown",
          summary: "Current review loop notes.",
          content:
            "# Agent sandbox\n\nThis folder isolates parallel work for implementation and review loops without requiring humans to manage worktrees by hand.",
        },
        {
          path: "artifacts",
          name: "artifacts",
          kind: "directory",
          parentPath: null,
          sizeLabel: "remote-only",
          updatedAt: "1 hour ago",
          hydrationState: "remote-only",
          summary: "Large generated outputs available on demand.",
        },
      ],
      revisions: [
        {
          id: "rev-agent-452",
          label: "auto: review notes",
          message: "Reviewer feedback and implementation context captured.",
          kind: "auto",
          createdAt: "12 minutes ago",
          author: "reviewer",
          changedFiles: 4,
          pinned: false,
        },
        {
          id: "rev-agent-440",
          label: "checkpoint: clean workspace",
          message: "Known-good agent sandbox before PR loop.",
          kind: "checkpoint",
          createdAt: "2 hours ago",
          author: "Bindhub desktop",
          changedFiles: 18,
          pinned: true,
        },
      ],
      settings: {
        syncState: "live",
        cachePolicy: "agent-sandbox",
        includeGitMetadata: false,
        allowAgentSandboxes: true,
        protectSecrets: true,
        visibility: "team",
      },
      recentActivity: [
        {
          id: "activity-agent-review",
          title: "Reviewer requested UI browse depth",
          detail: "Folder content needs to be visible from the dashboard.",
          timestamp: "12 minutes ago",
        },
        {
          id: "activity-agent-sync",
          title: "Partial hydration retained",
          detail: "Remote-only artifacts are visible without occupying disk.",
          timestamp: "1 hour ago",
        },
      ],
      readme:
        "Agent sandbox is for parallel work without forcing humans to manage worktrees manually. The folder can keep source material visible while heavy artifacts stay remote-only until explicitly opened or warmed.",
    },
  ]
  const machines: MachineSummary[] = [
    {
      id: "machine-current-browser",
      displayName: "This browser session",
      accountId: identity.account.accountId,
      trustState: "trusted",
      lastSeen: "Active now",
    },
    {
      id: "machine-cli-waiting",
      displayName: "CLI pairing placeholder",
      accountId: identity.account.accountId,
      trustState: "pending",
      lastSeen: "Ready for browser auth",
    },
  ]

  return {
    identity,
    source,
    overview: overview(folders, machines),
    folders,
    machines,
  }
}

function emptyHostedTree(): SharedFolderTreeWire {
  return {
    revision_id: null,
    file_count: 0,
    entries: [],
    revisions: [],
  }
}

function hostedEntriesFromTree(tree: SharedFolderTreeWire): SharedFolderEntry[] {
  return tree.entries
    .filter(
      (
        entry
      ): entry is SharedFolderTreeEntryWire & { kind: "file" | "directory" } =>
        entry.kind === "file" || entry.kind === "directory"
    )
    .map((entry) => ({
      path: entry.path,
      name: entry.name,
      kind: entry.kind,
      parentPath: entry.parent_path,
      sizeLabel:
        entry.kind === "directory" ? "folder" : formatBytes(entry.size_bytes ?? 0),
      updatedAt: entry.updated_at,
      hydrationState: "remote-only",
      language: languageForPath(entry.path),
      summary:
        entry.kind === "directory"
          ? "Folder in the latest hosted revision."
          : "File in the latest hosted revision.",
    }))
}

function hostedRevisionsFromTree(tree: SharedFolderTreeWire): FolderRevision[] {
  return tree.revisions.map((revision) => ({
    id: revision.id,
    label: revision.id,
    message: `Loom ${revision.boundary} revision`,
    kind: "auto",
    createdAt: revision.created_at,
    author: "Bindhub hosted",
    changedFiles: revision.changed_files,
    pinned: false,
  }))
}

function hostedReadmeFromTree(displayName: string, tree: SharedFolderTreeWire) {
  if (!tree.revision_id) {
    return `${displayName} is known to Bindhub, but no synced folder revision is available yet.`
  }
  return `${displayName} is backed by hosted Loom revision ${tree.revision_id}.`
}

function latestTreeTimestamp(tree: SharedFolderTreeWire) {
  return tree.revisions.at(-1)?.created_at ?? tree.entries[0]?.updated_at ?? null
}

function formatBytes(bytes: number) {
  if (bytes >= 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GiB`
  }
  if (bytes >= 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`
  }
  if (bytes >= 1024) {
    return `${(bytes / 1024).toFixed(1)} KiB`
  }
  return `${bytes} bytes`
}

function languageForPath(path: string) {
  const extension = path.split(".").pop()?.toLowerCase()
  switch (extension) {
    case "astro":
      return "Astro"
    case "css":
      return "CSS"
    case "go":
      return "Go"
    case "html":
      return "HTML"
    case "js":
    case "jsx":
      return "JavaScript"
    case "json":
      return "JSON"
    case "md":
    case "mdx":
      return "Markdown"
    case "rs":
      return "Rust"
    case "ts":
    case "tsx":
      return "TypeScript"
    case "toml":
      return "TOML"
    case "yml":
    case "yaml":
      return "YAML"
    default:
      return undefined
  }
}

function hostedPlaceholderSettings(
  visibility: SharedFolderSummary["visibility"]
): SharedFolderSettings {
  return {
    syncState: "live",
    cachePolicy: "online-first",
    includeGitMetadata: false,
    allowAgentSandboxes: true,
    protectSecrets: true,
    visibility,
  }
}

function overview(
  folders: Array<Pick<SharedFolderSummary, "id"> | SharedFolderWire>,
  machines: Array<Pick<MachineSummary, "trustState"> | DeviceWire>
): DashboardOverview {
  return {
    folderCount: folders.length,
    machineCount: machines.length,
    trustedMachineCount: machines.filter(
      (machine) =>
        !("trustState" in machine) || machine.trustState === "trusted"
    ).length,
  }
}

function normalizeMode(value: string | undefined): DashboardDataMode {
  if (
    value === "hosted-workos" ||
    value === "local-dev-api" ||
    value === "local-dev-fixtures"
  ) {
    return value
  }

  return process.env.NODE_ENV === "production"
    ? "hosted-workos"
    : "local-dev-fixtures"
}

function requireUrl(value: string | undefined, name: string): string {
  if (!value) {
    throw new Error(`${name} is required for the selected dashboard data mode`)
  }

  return value.replace(/\/+$/, "")
}

function requiredEnv(name: string): string {
  const value = process.env[name]?.trim()

  if (!value) {
    throw new Error(`${name} is required for local dev API mode`)
  }

  return value
}
