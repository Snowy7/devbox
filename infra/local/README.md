# Local Infrastructure

Optional local development services live here.

Expected services:

- Postgres for backend metadata integration tests. Run `scripts/test-postgres-metadata.sh` with
  Docker/OrbStack, or set `BINDHUB_TEST_POSTGRES_URL` to an existing database.
- MinIO for exercising the S3-compatible object storage boundary before wiring Cloudflare R2.

Default tests do not require Docker, MinIO, R2, AWS, or network credentials.

When an integration environment is added, MinIO should expose an S3-compatible endpoint that can be
passed to:

```text
bindhub sync remote check \
  --remote-kind s3 \
  --s3-endpoint http://127.0.0.1:9000 \
  --s3-bucket bindhub-local \
  --s3-region us-east-1 \
  --s3-access-key-env BINDHUB_MINIO_ACCESS_KEY \
  --s3-secret-key-env BINDHUB_MINIO_SECRET_KEY
```

Use `--validate-only` to verify CLI config and redaction without a network request.
