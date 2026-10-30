FROM rust:1.98-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations

RUN cargo build --release -p hookrelay-bin

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home hookrelay

COPY --from=builder /app/target/release/webhookd /usr/local/bin/webhookd

USER hookrelay
EXPOSE 8090

HEALTHCHECK --interval=15s --timeout=3s --start-period=10s \
    CMD curl -fsS http://127.0.0.1:8090/healthz || exit 1

ENTRYPOINT ["webhookd"]
CMD ["serve"]
