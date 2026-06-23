import { createFileRoute } from "@tanstack/react-router"
import { ArrowRight } from "lucide-react"

import { Input } from "@workspace/ui/components/input"
import { Label } from "@workspace/ui/components/label"

import {
  AuthError,
  AuthScreen,
  AuthSubmitButton,
  GoogleAuthButton,
} from "@/components/auth-screen"
import { safeReturnPathname } from "@/lib/auth"
import {
  passwordAuthFailureResponse,
  signUpWithPassword,
} from "@/lib/custom-auth"

export const Route = createFileRoute("/auth/sign-up")({
  validateSearch: (search: Record<string, unknown>) => ({
    error: typeof search.error === "string" ? search.error : undefined,
    returnPathname:
      typeof search.returnPathname === "string"
        ? search.returnPathname
        : undefined,
  }),
  server: {
    handlers: {
      POST: async ({ request }: { request: Request }) => {
        const form = await request.formData()
        const name = String(form.get("name") ?? "").trim()
        const email = String(form.get("email") ?? "").trim()
        const password = String(form.get("password") ?? "")
        const returnPathname =
          safeReturnPathname(String(form.get("returnPathname") ?? "")) ?? "/"

        if (!name || !email || !password) {
          return passwordAuthFailureResponse(
            request,
            "sign-up",
            "missing_fields",
            returnPathname
          )
        }

        const result = await signUpWithPassword({
          request,
          name,
          email,
          password,
          returnPathname,
        })

        if (!result.ok) {
          return passwordAuthFailureResponse(
            request,
            "sign-up",
            result.error,
            returnPathname
          )
        }

        return result.response
      },
    },
  },
  component: SignUpPage,
})

function SignUpPage() {
  const search = Route.useSearch()
  const returnPathname =
    safeReturnPathname(search.returnPathname ?? null) ?? "/"
  const error = errorMessage(search.error ?? null)
  const googleHref = `/auth/google?returnPathname=${encodeURIComponent(returnPathname)}`

  return (
    <AuthScreen
      title="Built for folders that follow your work."
      description="Create the account your machines sync against, then connect folders, approve CLI sessions, and browse everything from one place."
      asideTitle="Get started"
      asideDescription="This account becomes the trust boundary for your shared folders and machines."
    >
      <div className="space-y-5">
        <form method="post" className="space-y-4">
          <input type="hidden" name="returnPathname" value={returnPathname} />
          {error ? <AuthError message={error} /> : null}
          <Field id="name" label="Name" name="name" autoComplete="name" />
          <Field
            id="email"
            label="Email"
            name="email"
            type="email"
            autoComplete="email"
          />
          <Field
            id="password"
            label="Password"
            name="password"
            type="password"
            autoComplete="new-password"
            minLength={8}
          />
          <AuthSubmitButton>
            Create account
            <ArrowRight />
          </AuthSubmitButton>
        </form>

        <div className="relative">
          <div className="absolute inset-0 flex items-center">
            <span className="w-full border-t" />
          </div>
          <div className="relative flex justify-center text-xs uppercase">
            <span className="bg-background px-2 text-muted-foreground">or</span>
          </div>
        </div>

        <GoogleAuthButton href={googleHref} />

        <p className="text-sm text-muted-foreground">
          Already have an account?{" "}
          <a
            className="font-medium text-foreground underline-offset-4 hover:underline"
            href={`/auth/sign-in?returnPathname=${encodeURIComponent(returnPathname)}`}
          >
            Sign in
          </a>
        </p>
      </div>
    </AuthScreen>
  )
}

function Field({
  id,
  label,
  type = "text",
  ...props
}: {
  id: string
  label: string
  name: string
  type?: string
  autoComplete: string
  minLength?: number
}) {
  return (
    <div className="space-y-2">
      <Label htmlFor={id}>{label}</Label>
      <Input
        id={id}
        type={type}
        required
        className="h-11"
        {...props}
      />
    </div>
  )
}

function errorMessage(error: string | null) {
  if (!error) {
    return undefined
  }

  return (
    {
      account_unavailable: "That account could not be created.",
      invalid_credentials: "Account created, but sign-in failed.",
      missing_fields: "Name, email, and password are required.",
      oauth_failed: "Google sign-in failed.",
      oauth_unavailable: "Google sign-in is not configured correctly yet.",
      oauth_state: "Google sign-in expired. Try again.",
      session_unavailable: "WorkOS did not return a session.",
    }[error] ?? "Account creation failed."
  )
}
