FROM rust:1.88-bookworm AS builder

WORKDIR /app
COPY . .
RUN cargo build --release --locked -p bindhub-metadata

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && mkdir -p /data \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/bindhub-metadata /usr/local/bin/bindhub-metadata

ENV BINDHUB_ALLOW_MOCK_AUTH=false

EXPOSE 8787

CMD ["bindhub-metadata"]
