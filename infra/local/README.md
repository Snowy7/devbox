# Local Infrastructure

Future local development services will live here.

Expected services:

- Postgres for backend metadata integration tests once the hosted service exists.
- MinIO for exercising the S3-compatible object storage boundary before wiring Cloudflare R2.

Default tests do not require Docker, MinIO, R2, AWS, or network credentials.

When an integration environment is added, MinIO should expose an S3-compatible endpoint that can be
passed to:

```text
devbox sync remote check \
  --remote-kind s3 \
  --s3-endpoint http://127.0.0.1:9000 \
  --s3-bucket devbox-local \
  --s3-region us-east-1 \
  --s3-access-key-env DEVBOX_MINIO_ACCESS_KEY \
  --s3-secret-key-env DEVBOX_MINIO_SECRET_KEY
```

Use `--validate-only` to verify CLI config and redaction without a network request.
