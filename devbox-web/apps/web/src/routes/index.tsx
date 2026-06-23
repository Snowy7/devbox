import { createFileRoute } from "@tanstack/react-router"
import { Button } from "@workspace/ui/components/button"

export const Route = createFileRoute("/")({ component: App })

function App() {
  return (
    <div className="flex min-h-svh items-center p-6">
      <div className="flex max-w-xl min-w-0 flex-col gap-5">
        <p className="text-sm text-muted-foreground">Devbox dashboard</p>
        <div className="space-y-3">
          <h1 className="text-3xl font-semibold tracking-normal">
            Folder continuity for developers.
          </h1>
          <p className="leading-7 text-muted-foreground">
            This TanStack Start app will become the authenticated Devbox
            dashboard for personal folders, public shared folders, devices, and
            hosted Loom state.
          </p>
        </div>
        <div className="flex gap-3">
          <Button>Open dashboard</Button>
          <Button variant="outline">Sign in</Button>
        </div>
      </div>
    </div>
  )
}
