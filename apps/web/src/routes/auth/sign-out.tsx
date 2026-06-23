import { createFileRoute } from "@tanstack/react-router"
import { signOut } from "@workos/authkit-tanstack-react-start"

export const Route = createFileRoute("/auth/sign-out")({
  loader: async () => {
    await signOut({ data: { returnTo: "/" } })
  },
})
