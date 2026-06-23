import { Link, createFileRoute } from "@tanstack/react-router"
import { Activity, FolderOpen, Laptop } from "lucide-react"

import { AppShell } from "@/components/app-shell"
import {
  ActionLink,
  EmptyState,
  FeedItem,
  FeedList,
  Panel,
  PanelHeader,
} from "@/components/ui-primitives"
import { loadAuthenticatedDashboard } from "@/lib/dashboard-loader"
import { trustIcon, trustIconClass } from "@/lib/ui-icons"

export const Route = createFileRoute("/")({
  loader: async ({ location }) => {
    return loadAuthenticatedDashboard(location.pathname)
  },
  component: HomePage,
})

function HomePage() {
  const data = Route.useLoaderData()

  const feedItems = data.folders.flatMap((folder) =>
    folder.recentActivity.slice(0, 1).map((activity) => ({
      ...activity,
      folderId: folder.id,
      folderName: folder.displayName,
    }))
  )

  return (
    <AppShell data={data} title="Home">
      <div className="shell-gap grid min-h-full grid-cols-1 pt-4 xl:grid-cols-[minmax(0,1fr)_var(--token-rail-width)]">
        <Panel>
          <PanelHeader
            icon={Activity}
            title="Recent activity"
            description="Updates across your shared folders."
            action={<ActionLink to="/folders">See all</ActionLink>}
          />
          {feedItems.length > 0 ? (
            <FeedList>
              {feedItems.map((item) => (
                <FeedItem
                  key={item.id}
                  icon={FolderOpen}
                  title={
                    <Link
                      to="/folders/$folderId"
                      params={{ folderId: item.folderId }}
                      search={{ tab: undefined, file: undefined }}
                      className="hover:text-signal hover:underline"
                    >
                      {item.title}
                    </Link>
                  }
                  meta={`${item.folderName} · ${item.timestamp}`}
                  description={item.detail}
                />
              ))}
            </FeedList>
          ) : (
            <EmptyState
              icon={Activity}
              message="No recent folder activity yet. Open a shared folder on this machine to get started."
            />
          )}
        </Panel>

        <aside>
          <Panel markers={false}>
            <PanelHeader
              icon={Laptop}
              title="Machines"
              action={<ActionLink to="/machines">View all</ActionLink>}
            />
            <FeedList>
              {data.machines.slice(0, 5).map((machine) => {
                const TrustIcon = trustIcon(machine.trustState)

                return (
                  <FeedItem
                    key={machine.id}
                    icon={Laptop}
                    title={machine.displayName}
                    meta={machine.lastSeen}
                    description={
                      <span className="inline-flex items-center gap-1.5 capitalize">
                        <TrustIcon
                          className={`size-3 ${trustIconClass(machine.trustState)}`}
                        />
                        {machine.trustState}
                      </span>
                    }
                  />
                )
              })}
            </FeedList>
          </Panel>
        </aside>
      </div>
    </AppShell>
  )
}
