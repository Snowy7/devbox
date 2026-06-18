# Local Infrastructure

Future local development services will live here.

Expected services:

- Postgres for backend metadata integration tests once the hosted service exists.
- MinIO for exercising the S3-compatible object storage boundary before wiring Cloudflare R2.

This PR intentionally does not add a compose stack. The MVP skeleton does not need long-running local services yet, and Rust local snapshot/restore work should stay runnable without external infrastructure.
