import { createFileRoute } from "@tanstack/react-router"
import { ArrowRight, Building2, KeyRound, Mail, WandSparkles } from "lucide-react"
import type { ReactNode } from "react"

import { Button } from "@workspace/ui/components/button"
import { Input } from "@workspace/ui/components/input"
import { Label } from "@workspace/ui/components/label"

import {
  AuthError,
  AuthScreen,
  GoogleAuthButton,
} from "@/components/auth-screen"
import { safeReturnPathname } from "@/lib/auth"
import {
  authenticateWithOrganizationSelection,
  checkEmailForSso,
  passwordAuthFailureResponse,
  sendMagicAuthCode,
  signInWithPassword,
  verifyMagicAuthCode,
  type WorkOsOrganizationOption,
} from "@/lib/custom-auth"

type SignInStep = "email" | "password" | "magic" | "org"

export const Route = createFileRoute("/auth/sign-in")({
  validateSearch: (search: Record<string, unknown>) => ({
    email: typeof search.email === "string" ? search.email : undefined,
    error: typeof search.error === "string" ? search.error : undefined,
    orgs: typeof search.orgs === "string" ? search.orgs : undefined,
    pending: typeof search.pending === "string" ? search.pending : undefined,
    returnPathname:
      typeof search.returnPathname === "string"
        ? search.returnPathname
        : undefined,
    step: isSignInStep(search.step) ? search.step : undefined,
  }),
  server: {
    handlers: {
      POST: async ({ request }: { request: Request }) => {
        const form = await request.formData()
        const intent = String(form.get("intent") ?? "")
        const email = String(form.get("email") ?? "").trim()
        const password = String(form.get("password") ?? "")
        const code = String(form.get("code") ?? "").trim()
        const pendingAuthenticationToken = String(form.get("pending") ?? "")
        const organizationId = String(form.get("organizationId") ?? "")
        const returnPathname =
          safeReturnPathname(String(form.get("returnPathname") ?? "")) ?? "/"

        if (intent === "check-email") {
          if (!email) {
            return redirectToSignIn(request, {
              error: "missing_email",
              returnPathname,
            })
          }

          const result = await checkEmailForSso({
            request,
            email,
            returnPathname,
          })

          if (result.method === "sso") {
            return result.response
          }

          return redirectToSignIn(request, {
            email,
            returnPathname,
            step: "password",
          })
        }

        if (intent === "password") {
          if (!email || !password) {
            return redirectToSignIn(request, {
              email,
              error: "missing_fields",
              returnPathname,
              step: "password",
            })
          }

          const result = await signInWithPassword({
            request,
            email,
            password,
            returnPathname,
          })

          if (!result.ok) {
            return passwordAuthFailureResponse(
              request,
              "sign-in",
              result.error,
              returnPathname
            )
          }

          return result.response
        }

        if (intent === "magic-send") {
          if (!email) {
            return redirectToSignIn(request, {
              error: "missing_email",
              returnPathname,
            })
          }

          const result = await sendMagicAuthCode({ email })

          if (!result.ok) {
            return redirectToSignIn(request, {
              email,
              error: result.error,
              returnPathname,
              step: "password",
            })
          }

          return redirectToSignIn(request, {
            email,
            returnPathname,
            step: "magic",
          })
        }

        if (intent === "magic-verify") {
          if (!email || !code) {
            return redirectToSignIn(request, {
              email,
              error: "missing_magic_code",
              returnPathname,
              step: "magic",
            })
          }

          const result = await verifyMagicAuthCode({
            request,
            email,
            code,
            returnPathname,
          })

          if (!result.ok) {
            return redirectToSignIn(request, {
              email,
              error: result.error,
              returnPathname,
              step: "magic",
            })
          }

          return result.response
        }

        if (intent === "org-select") {
          if (!pendingAuthenticationToken || !organizationId) {
            return redirectToSignIn(request, {
              error: "missing_org",
              returnPathname,
              step: "org",
            })
          }

          const result = await authenticateWithOrganizationSelection({
            request,
            pendingAuthenticationToken,
            organizationId,
            returnPathname,
          })

          if (!result.ok) {
            return redirectToSignIn(request, {
              error: result.error,
              returnPathname,
              step: "org",
            })
          }

          return result.response
        }

        return redirectToSignIn(request, {
          error: "auth_failed",
          returnPathname,
        })
      },
    },
  },
  component: SignInPage,
})

function SignInPage() {
  const search = Route.useSearch()
  const returnPathname =
    safeReturnPathname(search.returnPathname ?? null) ?? "/"
  const step: SignInStep = search.step ?? (search.email ? "password" : "email")
  const error = errorMessage(search.error ?? null)
  const email = search.email ?? ""
  const organizations = parseOrganizations(search.orgs)
  const googleHref = `/auth/google?returnPathname=${encodeURIComponent(returnPathname)}`

  return (
    <AuthScreen
      title="Built for developers who switch machines."
      description="Sign in once and your shared folders, trusted machines, and browser-approved CLI sessions stay tied to the same account."
      asideTitle={step === "email" ? "Sign in" : step === "org" ? "Choose org" : "Welcome back"}
      asideDescription={
        step === "email"
          ? "Start with your email. Bindhub will use SSO when your domain requires it."
          : step === "org"
            ? "WorkOS needs one more choice before creating your Bindhub session."
            : "Use your password, a magic code, or continue through Google."
      }
    >
      <div className="space-y-5">
        {error ? <AuthError message={error} /> : null}

        {step === "email" ? (
          <EmailStep returnPathname={returnPathname} />
        ) : step === "magic" ? (
          <MagicStep email={email} returnPathname={returnPathname} />
        ) : step === "org" ? (
          <OrganizationStep
            organizations={organizations}
            pending={search.pending ?? ""}
            returnPathname={returnPathname}
          />
        ) : (
          <PasswordStep email={email} returnPathname={returnPathname} />
        )}

        {step !== "org" ? (
          <>
            <Divider />
            <GoogleAuthButton href={googleHref} />
          </>
        ) : null}

        <p className="text-sm text-muted-foreground">
          New here?{" "}
          <a
            className="font-medium text-foreground underline-offset-4 hover:underline"
            href={`/auth/sign-up?returnPathname=${encodeURIComponent(returnPathname)}`}
          >
            Create an account
          </a>
        </p>
      </div>
    </AuthScreen>
  )
}

function EmailStep({ returnPathname }: { returnPathname: string }) {
  return (
    <form method="post" className="space-y-4">
      <input type="hidden" name="intent" value="check-email" />
      <input type="hidden" name="returnPathname" value={returnPathname} />
      <Field
        id="email"
        label="Email"
        name="email"
        type="email"
        autoComplete="email"
        icon={<Mail className="size-4" />}
      />
      <SubmitButton>
        Continue
        <ArrowRight />
      </SubmitButton>
    </form>
  )
}

function PasswordStep({
  email,
  returnPathname,
}: {
  email: string
  returnPathname: string
}) {
  return (
    <div className="space-y-4">
      <form method="post" className="space-y-4">
        <input type="hidden" name="intent" value="password" />
        <input type="hidden" name="returnPathname" value={returnPathname} />
        <Field
          id="email"
          label="Email"
          name="email"
          type="email"
          autoComplete="email"
          defaultValue={email}
          icon={<Mail className="size-4" />}
        />
        <Field
          id="password"
          label="Password"
          name="password"
          type="password"
          autoComplete="current-password"
          icon={<KeyRound className="size-4" />}
        />
        <SubmitButton>
          Sign in
          <ArrowRight />
        </SubmitButton>
      </form>

      <form method="post">
        <input type="hidden" name="intent" value="magic-send" />
        <input type="hidden" name="email" value={email} />
        <input type="hidden" name="returnPathname" value={returnPathname} />
        <Button type="submit" variant="outline" className="h-9 w-full">
          <WandSparkles />
          Send magic code instead
        </Button>
      </form>

      <a
        className="block text-sm font-medium text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
        href={`/auth/sign-in?returnPathname=${encodeURIComponent(returnPathname)}`}
      >
        Use another email
      </a>
    </div>
  )
}

function MagicStep({
  email,
  returnPathname,
}: {
  email: string
  returnPathname: string
}) {
  return (
    <div className="space-y-4">
      <form method="post" className="space-y-4">
        <input type="hidden" name="intent" value="magic-verify" />
        <input type="hidden" name="email" value={email} />
        <input type="hidden" name="returnPathname" value={returnPathname} />
        <p className="border border-divider bg-input px-3 py-2 text-sm text-muted-foreground">
          We sent a one-time code to{" "}
          <span className="font-medium text-foreground">{email}</span>.
        </p>
        <Field
          id="code"
          label="Magic code"
          name="code"
          type="text"
          autoComplete="one-time-code"
          icon={<WandSparkles className="size-4" />}
        />
        <SubmitButton>
          Verify code
          <ArrowRight />
        </SubmitButton>
      </form>

      <form method="post">
        <input type="hidden" name="intent" value="magic-send" />
        <input type="hidden" name="email" value={email} />
        <input type="hidden" name="returnPathname" value={returnPathname} />
        <Button type="submit" variant="outline" className="h-9 w-full">
          Resend code
        </Button>
      </form>
    </div>
  )
}

function OrganizationStep({
  organizations,
  pending,
  returnPathname,
}: {
  organizations: WorkOsOrganizationOption[]
  pending: string
  returnPathname: string
}) {
  return (
    <form method="post" className="space-y-4">
      <input type="hidden" name="intent" value="org-select" />
      <input type="hidden" name="pending" value={pending} />
      <input type="hidden" name="returnPathname" value={returnPathname} />
      <div className="space-y-2">
        {organizations.length > 0 ? (
          organizations.map((organization, index) => (
            <label
              key={organization.id}
              className="flex cursor-pointer items-center gap-3 border border-border bg-input p-3 text-sm transition-colors hover:bg-muted"
            >
              <input
                required
                defaultChecked={index === 0}
                type="radio"
                name="organizationId"
                value={organization.id}
              />
              <Building2 className="size-4 text-muted-foreground" />
              <span className="font-medium">{organization.name}</span>
            </label>
          ))
        ) : (
          <Field
            id="organizationId"
            label="Organization ID"
            name="organizationId"
            type="text"
            autoComplete="off"
            icon={<Building2 className="size-4" />}
          />
        )}
      </div>
      <SubmitButton>
        Continue
        <ArrowRight />
      </SubmitButton>
    </form>
  )
}

function Field({
  id,
  label,
  icon,
  ...props
}: {
  id: string
  label: string
  name: string
  type: string
  autoComplete: string
  defaultValue?: string
  icon?: ReactNode
}) {
  return (
    <div className="space-y-2">
      <Label htmlFor={id}>{label}</Label>
      <div className="relative">
        {icon ? (
          <span className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground">
            {icon}
          </span>
        ) : null}
        <Input
          id={id}
          required
          className={icon ? "pl-9" : undefined}
          {...props}
        />
      </div>
    </div>
  )
}

function SubmitButton({ children }: { children: ReactNode }) {
  return (
    <Button type="submit" className="h-9 w-full">
      {children}
    </Button>
  )
}

function Divider() {
  return (
    <div className="relative">
      <div className="absolute inset-0 flex items-center">
        <span className="w-full border-t" />
      </div>
      <div className="relative flex justify-center text-xs uppercase">
        <span className="bg-card px-2 text-label text-faint">
          or
        </span>
      </div>
    </div>
  )
}

function redirectToSignIn(
  request: Request,
  params: {
    email?: string
    error?: string
    returnPathname: string
    step?: SignInStep
  }
) {
  const target = new URL("/auth/sign-in", request.url)
  target.searchParams.set("returnPathname", params.returnPathname)

  if (params.email) {
    target.searchParams.set("email", params.email)
  }

  if (params.error) {
    target.searchParams.set("error", params.error)
  }

  if (params.step) {
    target.searchParams.set("step", params.step)
  }

  return new Response(null, {
    status: 303,
    headers: {
      Location: target.toString(),
    },
  })
}

function parseOrganizations(value: string | undefined) {
  if (!value) {
    return []
  }

  try {
    const parsed = JSON.parse(decodeURIComponent(value))
    return Array.isArray(parsed) ? (parsed as WorkOsOrganizationOption[]) : []
  } catch {
    return []
  }
}

function isSignInStep(value: unknown): value is SignInStep {
  return (
    value === "email" ||
    value === "password" ||
    value === "magic" ||
    value === "org"
  )
}

function errorMessage(error: string | null) {
  if (!error) {
    return undefined
  }

  return (
    {
      auth_failed: "Authentication failed.",
      invalid_credentials: "Email or password is wrong.",
      magic_send_failed: "Magic code could not be sent.",
      magic_verify_failed: "Magic code is wrong or expired.",
      missing_email: "Email is required.",
      missing_fields: "Email and password are required.",
      missing_magic_code: "Magic code is required.",
      missing_org: "Choose an organization.",
      oauth_failed: "Google sign-in failed.",
      oauth_unavailable: "Google sign-in is not configured correctly yet.",
      oauth_state: "Google sign-in expired. Try again.",
      org_selection_failed: "Organization selection failed.",
      session_unavailable: "WorkOS did not return a session.",
    }[error] ?? "Authentication failed."
  )
}
