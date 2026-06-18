# Private Alpha UX and Safety Completion

This PR completes the private-alpha MVP control/safety surface over the foundations that already
exist in the Rust crates.

## Completed For Alpha

- Electron desktop shell with no-network alpha state.
- Tray/status affordance for idle/warning/syncing/paused style states.
- Explicit path-scoped secret policy records for block, template, and envelope-reference decisions.
- Manual conflict listing/showing plus guarded resolution records.
- Redacted devices, pairing, recovery, rotation, and remote/provider visibility in CLI/UI surfaces.
- Headless validation for Rust and desktop code paths.

## Safety Rules

- Raw secret material, token hashes, sync keys, device keys, key-envelope plaintext, recovery
  secrets, object credentials, and provider secrets must not be printed in CLI output or UI fixtures.
- Secret detection remains block-by-default before blob-cache writes.
- Template and envelope policy records do not make raw detected bytes uploadable.
- Conflict resolution records do not apply automatic merges or writes.
- Desktop renderer code does not mutate workspace files directly.

## Deferred

The private-alpha completion does not include paid SaaS, team administration, agent workflows, Git
replacement semantics, live OAuth/OIDC provider UI, live Cloudflare/AWS credential provisioning,
production deployment hardening, hosted conflict service, or automatic conflict resolution.
