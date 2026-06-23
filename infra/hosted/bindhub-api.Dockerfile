FROM rust:1.88-bookworm AS builder

WORKDIR /app
COPY . .
RUN cargo build --release --locked -p bindhub-api

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && mkdir -p /tmp/bindhub-api \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/bindhub-api /usr/local/bin/bindhub-api

ENV BINDHUB_API_ROOT=/tmp/bindhub-api

EXPOSE 8787

CMD ["bindhub-api"]
