FROM rust:1.88-bookworm AS builder

WORKDIR /app
COPY . .
RUN cargo build --release --locked -p devbox-metadata

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && useradd --system --home /nonexistent --shell /usr/sbin/nologin devbox \
    && mkdir -p /data \
    && chown devbox:devbox /data \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/devbox-metadata /usr/local/bin/devbox-metadata

ENV DEVBOX_ALLOW_MOCK_AUTH=false

EXPOSE 8787

USER devbox

CMD ["devbox-metadata"]
