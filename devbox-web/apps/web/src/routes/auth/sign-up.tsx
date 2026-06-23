import { createFileRoute } from "@tanstack/react-router"
import { getSignUpUrl } from "@workos/authkit-tanstack-react-start"

import { defaultSignedInPath, safeReturnPathname } from "@/lib/auth"

export const Route = createFileRoute("/auth/sign-up")({
  server: {
    handlers: {
      GET: async ({ request }: { request: Request }) => {
        const returnPathname =
          safeReturnPathname(
            new URL(request.url).searchParams.get("returnPathname")
          ) ?? defaultSignedInPath
        const url = await getSignUpUrl({ data: { returnPathname } })

        return Response.redirect(url, 307)
      },
    },
  },
})
