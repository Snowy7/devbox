import { redirect } from "@tanstack/react-router"
import { createServerFn } from "@tanstack/react-start"

import { authRoutes } from "@/lib/auth"
import type { DashboardData } from "@/lib/dashboard-api"

type DashboardLoaderInput = {
  pathname: string
}

type DashboardLoaderResult =
  | {
      authenticated: true
      data: DashboardData
    }
  | {
      authenticated: false
    }

const loadAuthenticatedDashboardOnServer = createServerFn({ method: "GET" })
  .validator((data: DashboardLoaderInput) => data)
  .handler(async ({ data }): Promise<DashboardLoaderResult> => {
    void data.pathname

    const [{ getAuth }, { loadDashboardData }] = await Promise.all([
      import("@workos/authkit-tanstack-react-start"),
      import("@/lib/dashboard-api"),
    ])

    const auth = await getAuth()

    if (!auth.user) {
      return { authenticated: false }
    }

    return {
      authenticated: true,
      data: await loadDashboardData(auth),
    }
  })

export async function loadAuthenticatedDashboard(pathname: string) {
  const result = await loadAuthenticatedDashboardOnServer({
    data: { pathname },
  })

  if (!result.authenticated) {
    throw redirect({
      href: `${authRoutes.signIn}?returnPathname=${encodeURIComponent(pathname)}`,
    })
  }

  return result.data
}
