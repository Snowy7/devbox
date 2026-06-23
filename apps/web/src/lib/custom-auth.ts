import { WorkOS } from "@workos-inc/node"
import { getAuthkit } from "@workos/authkit-tanstack-react-start"

import {
  readWorkOsAuthEnv,
  safeReturnPathname,
} from "@/lib/auth"
import { readServerRuntimeEnv } from "@/lib/server-env"

const oauthStateCookie = "Bindhub-oauth-state"
const oauthReturnCookie = "Bindhub-oauth-return"

type AuthResult =
  | { ok: true; response: Response }
  | { ok: false; error: string; status?: number }

type EmailCheckResult =
  | { method: "credentials" }
  | { method: "sso"; response: Response }
  | { method: "error"; error: string }

type PasswordAuthInput = {
  request: Request
  email: string
  password: string
  returnPathname: string
}

type MagicAuthInput = {
  request: Request
  email: string
  code: string
  returnPathname: string
}

type SignUpInput = PasswordAuthInput & {
  name: string
}

type OrganizationSelectionInput = {
  request: Request
  pendingAuthenticationToken: string
  organizationId: string
  returnPathname: string
}

export type WorkOsOrganizationOption = {
  id: string
  name: string
}

type WorkOsAuthError = Error & {
  status?: number
  error?: string
  code?: string
  errorDescription?: string
  rawData?: {
    code?: string
    error?: string
    pending_authentication_token?: string
    organizations?: WorkOsOrganizationOption[]
    connection_ids?: string[]
    email?: string
  }
}

function readEnv() {
  const env = readWorkOsAuthEnv(readServerRuntimeEnv())

  if (!env.clientId || !env.apiKey || !env.cookiePassword) {
    throw new Error(
      "WorkOS auth is not configured. Set WORKOS_CLIENT_ID, WORKOS_API_KEY, and WORKOS_COOKIE_PASSWORD."
    )
  }

  return env
}

function workosClient() {
  const env = readEnv()

  return {
    env,
    workos: new WorkOS(env.apiKey, {
      clientId: env.clientId,
    }),
  }
}

function getRequestIp(request: Request): string | undefined {
  return (
    request.headers.get("x-forwarded-for")?.split(",")[0]?.trim() ||
    request.headers.get("x-real-ip") ||
    undefined
  )
}

function getUserAgent(request: Request): string | undefined {
  return request.headers.get("user-agent") ?? undefined
}

function sessionConfig(env: ReturnType<typeof readEnv>) {
  return {
    sealSession: true,
    cookiePassword: env.cookiePassword,
  }
}

function redirectToAuthError(
  request: Request,
  path: "/auth/sign-in" | "/auth/sign-up",
  error: string,
  returnPathname: string
) {
  const target = new URL(path, request.url)
  target.searchParams.set("error", error)
  target.searchParams.set("returnPathname", returnPathname)

  return redirectResponse(target)
}

async function saveSealedSession(
  request: Request,
  sealedSession: string,
  returnPathname: string
) {
  const authkit = await getAuthkit()
  const redirectUrl = new URL(returnPathname, request.url)
  const response = redirectResponse(redirectUrl)
  const { response: savedResponse, headers } = await authkit.saveSession(
    response,
    sealedSession
  )

  const finalResponse = savedResponse ?? response

  const cookieHeaders = headers?.["Set-Cookie"] ?? headers?.["set-cookie"]
  if (Array.isArray(cookieHeaders)) {
    for (const header of cookieHeaders) {
      finalResponse.headers.append("Set-Cookie", header)
    }
  } else if (cookieHeaders) {
    finalResponse.headers.append("Set-Cookie", cookieHeaders)
  }

  return finalResponse
}

export async function checkEmailForSso({
  request,
  email,
  returnPathname,
}: {
  request: Request
  email: string
  returnPathname: string
}): Promise<EmailCheckResult> {
  const domain = email.split("@")[1]?.toLowerCase()

  if (!domain) {
    return { method: "credentials" }
  }

  try {
    const { env, workos } = workosClient()
    const connections = await workos.sso.listConnections({ domain })
    const activeConnection = connections.data.find(
      (connection) => connection.state === "active"
    )

    if (!activeConnection) {
      return { method: "credentials" }
    }

    const url = new URL(request.url)
    const state = `bindhub_auth_${crypto.randomUUID()}`
    const callbackUrl = env.redirectUri ?? new URL("/auth/callback", url).href
    const authorizationUrl = workos.userManagement.getAuthorizationUrl({
      connectionId: activeConnection.id,
      clientId: env.clientId,
      redirectUri: callbackUrl,
      state,
    })
    const response = redirectResponse(authorizationUrl, 307)
    response.headers.append(
      "Set-Cookie",
      serializeCookie(oauthStateCookie, state, 600)
    )
    response.headers.append(
      "Set-Cookie",
      serializeCookie(oauthReturnCookie, returnPathname, 600)
    )

    return { method: "sso", response }
  } catch (error) {
    console.error("[Bindhub-web] email SSO check failed", error)
    return { method: "credentials" }
  }
}

export async function signInWithPassword({
  request,
  email,
  password,
  returnPathname,
}: PasswordAuthInput): Promise<AuthResult> {
  try {
    const { env, workos } = workosClient()
    const result = await workos.userManagement.authenticateWithPassword({
      clientId: env.clientId,
      email,
      password,
      ipAddress: getRequestIp(request),
      userAgent: getUserAgent(request),
      session: sessionConfig(env),
    })

    if (!result.sealedSession) {
      return { ok: false, error: "session_unavailable", status: 502 }
    }

    return {
      ok: true,
      response: await saveSealedSession(
        request,
        result.sealedSession,
        returnPathname
      ),
    }
  } catch (error) {
    const workosError = error as WorkOsAuthError
    const orgSelection = organizationSelectionFromError(
      workosError,
      request,
      returnPathname
    )
    if (orgSelection) {
      return { ok: true, response: orgSelection }
    }

    const ssoRedirect = ssoRequiredFromError(workosError, request, returnPathname)
    if (ssoRedirect) {
      return { ok: true, response: ssoRedirect }
    }

    console.error("[Bindhub-web] password sign-in failed", error)
    return { ok: false, error: "invalid_credentials", status: 401 }
  }
}

export async function sendMagicAuthCode({
  email,
}: {
  email: string
}): Promise<{ ok: true } | { ok: false; error: string }> {
  try {
    const { workos } = workosClient()
    await workos.userManagement.createMagicAuth({ email })
    return { ok: true }
  } catch (error) {
    console.error("[Bindhub-web] magic auth send failed", error)
    return { ok: false, error: "magic_send_failed" }
  }
}

export async function verifyMagicAuthCode({
  request,
  email,
  code,
  returnPathname,
}: MagicAuthInput): Promise<AuthResult> {
  try {
    const { env, workos } = workosClient()
    const result = await workos.userManagement.authenticateWithMagicAuth({
      clientId: env.clientId,
      email,
      code,
      ipAddress: getRequestIp(request),
      userAgent: getUserAgent(request),
      session: sessionConfig(env),
    })

    if (!result.sealedSession) {
      return { ok: false, error: "session_unavailable", status: 502 }
    }

    return {
      ok: true,
      response: await saveSealedSession(
        request,
        result.sealedSession,
        returnPathname
      ),
    }
  } catch (error) {
    const workosError = error as WorkOsAuthError
    const orgSelection = organizationSelectionFromError(
      workosError,
      request,
      returnPathname
    )
    if (orgSelection) {
      return { ok: true, response: orgSelection }
    }

    const ssoRedirect = ssoRequiredFromError(workosError, request, returnPathname)
    if (ssoRedirect) {
      return { ok: true, response: ssoRedirect }
    }

    console.error("[Bindhub-web] magic auth verify failed", error)
    return { ok: false, error: "magic_verify_failed", status: 401 }
  }
}

export async function authenticateWithOrganizationSelection({
  request,
  pendingAuthenticationToken,
  organizationId,
  returnPathname,
}: OrganizationSelectionInput): Promise<AuthResult> {
  try {
    const { env, workos } = workosClient()
    const result =
      await workos.userManagement.authenticateWithOrganizationSelection({
        clientId: env.clientId,
        pendingAuthenticationToken,
        organizationId,
        ipAddress: getRequestIp(request),
        userAgent: getUserAgent(request),
        session: sessionConfig(env),
      })

    if (!result.sealedSession) {
      return { ok: false, error: "session_unavailable", status: 502 }
    }

    return {
      ok: true,
      response: await saveSealedSession(
        request,
        result.sealedSession,
        returnPathname
      ),
    }
  } catch (error) {
    console.error("[Bindhub-web] organization selection failed", error)
    return { ok: false, error: "org_selection_failed", status: 401 }
  }
}

export async function signUpWithPassword({
  request,
  name,
  email,
  password,
  returnPathname,
}: SignUpInput): Promise<AuthResult> {
  try {
    const { workos } = workosClient()
    await workos.userManagement.createUser({
      email,
      password,
      name: name || undefined,
      ipAddress: getRequestIp(request),
      userAgent: getUserAgent(request),
    })
  } catch (error) {
    console.error("[Bindhub-web] user creation failed", error)
    return { ok: false, error: "account_unavailable", status: 400 }
  }

  return signInWithPassword({ request, email, password, returnPathname })
}

export function passwordAuthFailureResponse(
  request: Request,
  mode: "sign-in" | "sign-up",
  error: string,
  returnPathname: string
) {
  return redirectToAuthError(
    request,
    mode === "sign-in" ? "/auth/sign-in" : "/auth/sign-up",
    error,
    returnPathname
  )
}

export async function startGoogleOAuth(request: Request) {
  const url = new URL(request.url)
  const returnPathname =
    safeReturnPathname(url.searchParams.get("returnPathname")) ?? "/"

  try {
    const { env, workos } = workosClient()
    const state = `bindhub_auth_${crypto.randomUUID()}`
    const callbackUrl = env.redirectUri ?? new URL("/auth/callback", url).href
    const authorizationUrl = workos.userManagement.getAuthorizationUrl({
      provider: "GoogleOAuth",
      clientId: env.clientId,
      redirectUri: callbackUrl,
      state,
    })

    const response = redirectResponse(authorizationUrl, 307)
    response.headers.append(
      "Set-Cookie",
      serializeCookie(oauthStateCookie, state, 600)
    )
    response.headers.append(
      "Set-Cookie",
      serializeCookie(oauthReturnCookie, returnPathname, 600)
    )

    return response
  } catch (error) {
    console.error("[Bindhub-web] Google OAuth start failed", error)
    return redirectToAuthError(
      request,
      "/auth/sign-in",
      "oauth_unavailable",
      returnPathname
    )
  }
}

export async function startAuthkitInitiate(request: Request) {
  try {
    const { env, workos } = workosClient()
    const url = new URL(request.url)
    const state = `bindhub_auth_${crypto.randomUUID()}`
    const callbackUrl = env.redirectUri ?? new URL("/auth/callback", url).href
    const authorizationUrl = workos.userManagement.getAuthorizationUrl({
      provider: "authkit",
      clientId: env.clientId,
      redirectUri: callbackUrl,
      state,
    })
    const response = redirectResponse(authorizationUrl, 307)
    response.headers.append(
      "Set-Cookie",
      serializeCookie(oauthStateCookie, state, 600)
    )
    response.headers.append("Set-Cookie", serializeCookie(oauthReturnCookie, "/", 600))

    return response
  } catch (error) {
    console.error("[Bindhub-web] AuthKit initiate failed", error)
    return redirectToAuthError(request, "/auth/sign-in", "auth_failed", "/")
  }
}

export async function maybeHandleCustomOAuthCallback(request: Request) {
  const url = new URL(request.url)
  const code = url.searchParams.get("code")
  const state = url.searchParams.get("state")

  if (!code || !state?.startsWith("bindhub_auth_")) {
    return undefined
  }

  const cookies = parseCookies(request.headers.get("cookie"))
  const expectedState = cookies.get(oauthStateCookie)
  const returnPathname =
    safeReturnPathname(cookies.get(oauthReturnCookie) ?? null) ?? "/"

  if (state !== expectedState) {
    const target = new URL("/auth/sign-in", request.url)
    target.searchParams.set("error", "oauth_state")
    target.searchParams.set("returnPathname", returnPathname)
    return redirectResponse(target)
  }

  try {
    const { env, workos } = workosClient()
    const result = await workos.userManagement.authenticateWithCode({
      clientId: env.clientId,
      code,
      ipAddress: getRequestIp(request),
      userAgent: getUserAgent(request),
      session: sessionConfig(env),
    })

    if (!result.sealedSession) {
      throw new Error("WorkOS did not return a sealed session")
    }

    const response = await saveSealedSession(
      request,
      result.sealedSession,
      returnPathname
    )
    response.headers.append("Set-Cookie", clearCookie(oauthStateCookie))
    response.headers.append("Set-Cookie", clearCookie(oauthReturnCookie))

    return response
  } catch (error) {
    const orgSelection = organizationSelectionFromError(
      error as WorkOsAuthError,
      request,
      returnPathname
    )
    if (orgSelection) {
      const response = orgSelection
      response.headers.append("Set-Cookie", clearCookie(oauthStateCookie))
      response.headers.append("Set-Cookie", clearCookie(oauthReturnCookie))
      return response
    }

    console.error("[Bindhub-web] Google OAuth callback failed", error)
    const target = new URL("/auth/sign-in", request.url)
    target.searchParams.set("error", "oauth_failed")
    target.searchParams.set("returnPathname", returnPathname)
    const response = redirectResponse(target)
    response.headers.append("Set-Cookie", clearCookie(oauthStateCookie))
    response.headers.append("Set-Cookie", clearCookie(oauthReturnCookie))
    return response
  }
}

function organizationSelectionFromError(
  error: WorkOsAuthError,
  request: Request,
  returnPathname: string
): Response | undefined {
  const rawData = error.rawData ?? {}
  const isRequired =
    rawData.code === "organization_selection_required" ||
    error.error === "organization_selection_required" ||
    error.code === "organization_selection_required"

  if (!isRequired || !rawData.pending_authentication_token) {
    return undefined
  }

  const target = new URL("/auth/sign-in", request.url)
  target.searchParams.set("step", "org")
  target.searchParams.set("pending", rawData.pending_authentication_token)
  target.searchParams.set("returnPathname", returnPathname)
  target.searchParams.set(
    "orgs",
    encodeURIComponent(JSON.stringify(rawData.organizations ?? []))
  )

  return redirectResponse(target)
}

function ssoRequiredFromError(
  error: WorkOsAuthError,
  request: Request,
  returnPathname: string
): Response | undefined {
  const rawData = error.rawData ?? {}
  const isRequired =
    rawData.error === "sso_required" ||
    error.error === "sso_required" ||
    error.code === "sso_required"
  const connectionId = rawData.connection_ids?.[0]

  if (!isRequired || !connectionId) {
    return undefined
  }

  try {
    const { env, workos } = workosClient()
    const url = new URL(request.url)
    const state = `bindhub_auth_${crypto.randomUUID()}`
    const callbackUrl = env.redirectUri ?? new URL("/auth/callback", url).href
    const authorizationUrl = workos.userManagement.getAuthorizationUrl({
      connectionId,
      clientId: env.clientId,
      redirectUri: callbackUrl,
      state,
    })
    const response = redirectResponse(authorizationUrl, 307)
    response.headers.append(
      "Set-Cookie",
      serializeCookie(oauthStateCookie, state, 600)
    )
    response.headers.append(
      "Set-Cookie",
      serializeCookie(oauthReturnCookie, returnPathname, 600)
    )
    return response
  } catch (redirectError) {
    console.error("[Bindhub-web] required SSO redirect failed", redirectError)
    return undefined
  }
}

function redirectResponse(target: string | URL, status = 303) {
  return new Response(null, {
    status,
    headers: {
      Location: target.toString(),
    },
  })
}

function serializeCookie(name: string, value: string, maxAge: number) {
  return `${name}=${encodeURIComponent(value)}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${maxAge}`
}

function clearCookie(name: string) {
  return `${name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT`
}

function parseCookies(header: string | null) {
  const cookies = new Map<string, string>()

  if (!header) {
    return cookies
  }

  for (const part of header.split(";")) {
    const [rawName, ...rawValue] = part.trim().split("=")
    if (!rawName || rawValue.length === 0) {
      continue
    }

    cookies.set(rawName, decodeURIComponent(rawValue.join("=")))
  }

  return cookies
}
