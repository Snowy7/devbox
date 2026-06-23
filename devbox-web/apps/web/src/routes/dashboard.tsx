import { Outlet, createFileRoute, redirect } from "@tanstack/react-router"
import { getAuth } from "@workos/authkit-tanstack-react-start"

import { AppShell } from "@/components/app-shell"
import { authRoutes } from "@/lib/auth"
import { loadDashboardData } from "@/lib/dashboard-api"

export const Route = createFileRoute("/dashboard")({
  loader: async ({ location }) => {
    const auth = await getAuth()

    if (!auth.user) {
      throw redirect({
        href: `${authRoutes.signIn}?returnPathname=${encodeURIComponent(location.pathname)}`,
      })
    }

    return loadDashboardData(auth)
  },
  component: DashboardPage,
})

function DashboardPage() {
  const data = Route.useLoaderData()

  return (
    <AppShell data={data}>
      <Outlet />
    </AppShell>
  )
}
