import { Outlet, createFileRoute, useRouterState } from "@tanstack/react-router"
import { FolderOpen } from "lucide-react"

import { AppShell } from "@/components/app-shell"
import { FolderTable } from "@/components/dashboard-sections"
import { Panel, PanelHeader } from "@/components/ui-primitives"
import { loadAuthenticatedDashboard } from "@/lib/dashboard-loader"

export const Route = createFileRoute("/folders")({
  loader: async ({ location }) => loadAuthenticatedDashboard(location.pathname),
  component: FoldersPage,
})

function FoldersPage() {
  const dashboardData = Route.useLoaderData()
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  })

  if (pathname !== "/folders") {
    return <Outlet />
  }

  return (
    <AppShell data={dashboardData} title="Folders">
      <div className="pt-4">
        <Panel>
          <PanelHeader
            icon={FolderOpen}
            title="Shared folders"
            description="Folders available on your account."
          />
          <FolderTable folders={dashboardData.folders} />
        </Panel>
      </div>
    </AppShell>
  )
}
