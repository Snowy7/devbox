import { createFileRoute } from "@tanstack/react-router"
import { Laptop } from "lucide-react"

import { AppShell } from "@/components/app-shell"
import { MachineTable } from "@/components/dashboard-sections"
import { Panel, PanelHeader } from "@/components/ui-primitives"
import { loadAuthenticatedDashboard } from "@/lib/dashboard-loader"

export const Route = createFileRoute("/machines")({
  loader: async ({ location }) => loadAuthenticatedDashboard(location.pathname),
  component: MachinesPage,
})

function MachinesPage() {
  const dashboardData = Route.useLoaderData()

  return (
    <AppShell data={dashboardData} title="Machines">
      <div className="pt-4">
        <Panel>
          <PanelHeader
            icon={Laptop}
            title="Trusted devices"
            description="Machines connected to your Bindhub account."
          />
          <MachineTable machines={dashboardData.machines} />
        </Panel>
      </div>
    </AppShell>
  )
}