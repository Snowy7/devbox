import type { UserInfo } from "@workos/authkit-tanstack-react-start"

import { identityFromWorkOsAuth, type DevboxWebIdentity } from "@/lib/identity"

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
  role: "owner" | "editor" | "viewer"
  accountId: string
  hydrationState: "remote-only" | "partial" | "fully-local"
  lastCheckpoint: string
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
  identity: DevboxWebIdentity
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

type DevboxSessionWire = {
  account_id: string
  session_id: string
  session_token: string
  device_id: string
}

type DevboxApiSession = {
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
      "x-devbox-device-id": session.deviceId,
    })
  }

  if (source.mode === "local-dev-api") {
    const token = requiredEnv("DEVBOX_LOCAL_API_SESSION_TOKEN")
    const deviceId = requiredEnv("DEVBOX_LOCAL_API_DEVICE_ID")

    return dataFromApi(identity, source, {
      Authorization: `Bearer ${token}`,
      "x-devbox-device-id": deviceId,
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
  if (process.env.DEVBOX_LOCAL_DEV_CLI_AUTH !== "1") {
    throw new Error("local-dev CLI auth is not enabled")
  }

  const localIdentity =
    process.env.DEVBOX_LOCAL_DEV_AUTH_EMAIL?.trim() || "local-dev@example.test"

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
  const serviceToken = requiredEnv("DEVBOX_HOSTED_API_SERVICE_TOKEN")
  const response = await fetch(
    `${baseUrl}/v1/auth/cli-device-flow/${encodeURIComponent(userCode)}/approve`,
    {
      method: "POST",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
        "x-devbox-api-service-token": serviceToken,
      },
      body: JSON.stringify({
        user_id: identity.userId,
        session_id: identity.sessionId,
        organization_id: identity.organizationId,
      }),
    }
  )

  if (!response.ok) {
    throw new Error(`Devbox CLI auth approval failed: ${response.status}`)
  }

  const session = (await response.json()) as DevboxSessionWire

  return {
    accountId: session.account_id,
    sessionId: session.session_id,
    deviceId: session.device_id,
  }
}

function readCliAuthApiBaseUrl(env: NodeJS.ProcessEnv): string {
  return requireUrl(
    env.DEVBOX_HOSTED_API_URL?.trim() || env.DEVBOX_LOCAL_API_URL?.trim(),
    "DEVBOX_HOSTED_API_URL or DEVBOX_LOCAL_API_URL"
  )
}

export function readDashboardDataSource(
  env: NodeJS.ProcessEnv
): DashboardDataSource {
  const mode = normalizeMode(env.DEVBOX_DASHBOARD_DATA_MODE)
  const hostedBaseUrl = env.DEVBOX_HOSTED_API_URL?.trim()
  const localBaseUrl = env.DEVBOX_LOCAL_API_URL?.trim()

  if (mode === "hosted-workos") {
    return {
      mode,
      label: "Hosted API with WorkOS bearer auth",
      baseUrl: requireUrl(hostedBaseUrl, "DEVBOX_HOSTED_API_URL"),
    }
  }

  if (mode === "local-dev-api") {
    return {
      mode,
      label: "Local dev API session",
      baseUrl: requireUrl(localBaseUrl, "DEVBOX_LOCAL_API_URL"),
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
): Promise<DevboxApiSession> {
  const baseUrl = source.baseUrl
  const serviceToken = requiredEnv("DEVBOX_HOSTED_API_SERVICE_TOKEN")

  if (!baseUrl) {
    throw new Error("dashboard API base URL is not configured")
  }

  const response = await fetch(`${baseUrl}/v1/auth/workos-session`, {
    method: "POST",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
      "x-devbox-api-service-token": serviceToken,
    },
    body: JSON.stringify({
      user_id: auth.user.id,
      session_id: auth.sessionId,
      organization_id: auth.organizationId ?? null,
      device_id: `web-${auth.sessionId}`,
      device_display_name: "Devbox web session",
    }),
  })

  if (!response.ok) {
    throw new Error(`Devbox API session exchange failed: ${response.status}`)
  }

  const session = (await response.json()) as DevboxSessionWire

  return {
    sessionToken: session.session_token,
    deviceId: session.device_id,
  }
}

async function dataFromApi(
  identity: DevboxWebIdentity,
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

  return {
    identity,
    source,
    overview: overview(folders, machines),
    folders: folders.map((folder) => ({
      id: folder.id,
      displayName: folder.display_name,
      role: folder.role,
      accountId: folder.account_id,
      hydrationState: "fully-local",
      lastCheckpoint: "Synced from Devbox API",
    })),
    machines: machines.map((machine) => ({
      id: machine.id,
      displayName: machine.display_name,
      accountId: machine.account_id,
      trustState: "trusted",
      lastSeen: "Known to Devbox API",
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
    throw new Error(`Devbox API request failed: ${response.status}`)
  }

  return response.json() as Promise<T>
}

function fixtureDashboardData(
  identity: DevboxWebIdentity,
  source: DashboardDataSource
): DashboardData {
  const folders: SharedFolderSummary[] = [
    {
      id: "folder-primary-code",
      displayName: "Primary code folder",
      role: "owner",
      accountId: identity.account.accountId,
      hydrationState: "fully-local",
      lastCheckpoint: "Ready on this machine",
    },
    {
      id: "folder-agent-sandbox",
      displayName: "Agent sandbox",
      role: "editor",
      accountId: identity.account.accountId,
      hydrationState: "partial",
      lastCheckpoint: "Waiting for next checkpoint",
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
