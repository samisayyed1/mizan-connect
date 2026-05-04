# syntax=docker/dockerfile:1.7
# ---- planner ----
FROM rust:1.95-slim-bookworm AS chef
WORKDIR /app
RUN cargo install cargo-chef --version 0.1.71 --locked

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---- builder ----
FROM chef AS builder
ENV SQLX_OFFLINE=true
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin mizan-connect \
    && strip target/release/mizan-connect

# ---- runtime ----
FROM gcr.io/distroless/cc-debian12 AS runtime
WORKDIR /app

# Non-root user (UID 1000 — distroless ships a `nonroot` user at 65532; we
# instead embed an etc/passwd entry via the static binary's expectations).
USER nonroot:nonroot

COPY --from=builder /app/target/release/mizan-connect /app/mizan-connect
COPY --from=builder /app/migrations /app/migrations

ENV APP_HOST=0.0.0.0 \
    APP_PORT=8080 \
    APP_ENV=production \
    LOG_FORMAT=json

EXPOSE 8080

# NOTE: distroless has no shell/curl, so the in-image HEALTHCHECK is
# omitted. The orchestrator (Fly.io) hits /health externally — see
# fly.toml's [[services.tcp_checks]] / [[http_service.checks]].

ENTRYPOINT ["/app/mizan-connect"]
