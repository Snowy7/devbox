import { createFileRoute } from "@tanstack/react-router"
import { handleCallbackRoute } from "@workos/authkit-tanstack-react-start"

import { safeReturnPathname } from "@/lib/auth"
import { maybeHandleCustomOAuthCallback } from "@/lib/custom-auth"

export const Route = createFileRoute("/auth/callback")({
  server: {
    handlers: {
      GET: async ({ request }: { request: Request }) => {
        const customOAuthResponse = await maybeHandleCustomOAuthCallback(request)
        if (customOAuthResponse) {
          return customOAuthResponse
        }

        const returnPathname = safeReturnPathname(
          new URL(request.url).searchParams.get("returnPathname")
        )

        return handleCallbackRoute({
          errorRedirectUrl: "/auth/sign-in?error=auth_failed",
          ...(returnPathname ? { returnPathname } : {}),
        })({ request })
      },
    },
  },
})
