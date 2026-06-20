# Private Alpha UX and Safety Completion

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Devbox.

This PR completes the private-alpha MVP control/safety surface over the foundations that already
exist in the Rust crates.

## Completed For Alpha

- Electron desktop shell with redacted `DEVBOX_*` alpha state and safe placeholder fallback.
- Tray/status affordance for ready/needs-config/command-only/blocked style states.
- Local DB/cache/project/receiver path visibility without workspace mutation from renderer code.
- Hosted API/session/shared-folder scope visibility; object leases, buckets, prefixes, and remote
  kinds stay out of product UX.
- Generated command-state for hosted login/status, object-access resolution, pairing, live sync,
  packaging, and deterministic smoke testing.
- Deterministic local two-device smoke harness with redacted evidence logs.
- macOS/Linux alpha release scripts for command-line tools and an unsigned desktop bundle.
- Explicit path-scoped secret policy records for block, template, and envelope-reference decisions.
- Manual conflict listing/showing plus guarded resolution records.
- Redacted devices, pairing, recovery, rotation, and remote/provider visibility in CLI/UI surfaces.
- Headless validation for Rust and desktop code paths.

## Safety Rules

- Raw secret material, token hashes, sync keys, device keys, key-envelope plaintext, recovery
  secrets, object credentials, and provider secrets must not be printed in CLI output or UI fixtures.
- The desktop may report whether an env var is configured, but must not render the value of session
  tokens, invite codes, pairing payloads, provider keys, or provider secrets.
- Evidence logs from alpha harnesses must redact secret-bearing pairing/session/cloud payloads.
- Secret detection remains block-by-default before blob-cache writes.
- Template and envelope policy records do not make raw detected bytes uploadable.
- Conflict resolution records do not apply automatic merges or writes.
- Desktop renderer code does not mutate workspace files directly.

## Deferred

The private-alpha completion does not include paid SaaS, team administration, agent workflows, Git
replacement semantics, live OAuth/OIDC provider UI, live Cloudflare/AWS credential provisioning,
signed installers, multi-region deployment hardening, hosted conflict service, or automatic conflict
resolution.
