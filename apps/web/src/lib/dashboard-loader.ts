import { redirect } from "@tanstack/react-router"
import { getAuth } from "@workos/authkit-tanstack-react-start"

import { authRoutes } from "@/lib/auth"
import { loadDashboardData } from "@/lib/dashboard-api"

export async function loadAuthenticatedDashboard(pathname: string) {
  const auth = await getAuth()

  if (!auth.user) {
    throw redirect({
      href: `${authRoutes.signIn}?returnPathname=${encodeURIComponent(pathname)}`,
    })
  }

  return loadDashboardData(auth)
}
