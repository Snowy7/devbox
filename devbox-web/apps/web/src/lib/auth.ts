export const authRoutes = {
  signIn: "/auth/sign-in",
  signUp: "/auth/sign-up",
  callback: "/auth/callback",
  signOut: "/auth/sign-out",
} as const

export const defaultSignedInPath = "/dashboard"

export function safeReturnPathname(value: string | null): string | undefined {
  if (!value || !value.startsWith("/") || value.startsWith("//")) {
    return undefined
  }

  return value
}

export type WorkOsAuthEnv = {
  clientId?: string
  apiKey?: string
  cookiePassword?: string
  redirectUri?: string
  signOutRedirectUri?: string
}

export function readWorkOsAuthEnv(env: NodeJS.ProcessEnv): WorkOsAuthEnv {
  return {
    clientId: env.WORKOS_CLIENT_ID,
    apiKey: env.WORKOS_API_KEY,
    cookiePassword: env.WORKOS_COOKIE_PASSWORD,
    redirectUri: env.WORKOS_REDIRECT_URI,
    signOutRedirectUri: env.WORKOS_SIGN_OUT_REDIRECT_URI,
  }
}
