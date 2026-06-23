import type { UserInfo } from "@workos/authkit-tanstack-react-start"

export type DevboxAccountIdentity = {
  accountId: string
  email: string
  displayName: string
  avatarUrl: string | null
}

export type DevboxOrgIdentity = {
  organizationId: string
  role: string | null
}

export type DevboxWebIdentity = {
  account: DevboxAccountIdentity
  organization: DevboxOrgIdentity | null
  sessionId: string
}

export function identityFromWorkOsAuth(auth: UserInfo): DevboxWebIdentity {
  return {
    account: {
      accountId: auth.user.id,
      email: auth.user.email,
      displayName: displayName(
        auth.user.firstName,
        auth.user.lastName,
        auth.user.email
      ),
      avatarUrl: auth.user.profilePictureUrl,
    },
    organization: auth.organizationId
      ? {
          organizationId: auth.organizationId,
          role: auth.role ?? null,
        }
      : null,
    sessionId: auth.sessionId,
  }
}

function displayName(
  firstName: string | null,
  lastName: string | null,
  email: string
) {
  const name = [firstName, lastName].filter(Boolean).join(" ").trim()

  return name || email
}
