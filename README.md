# Mizan Connect

Proprietary backend for [Mizan](https://github.com/samisayyed1/mizan-4) — a desktop investment tracker.

> **License:** All Rights Reserved. Proprietary. Not for distribution.

## Overview

Mizan Connect is a Rust + Axum HTTP service that handles cross-device concerns for the Mizan desktop client:

| Capability | Status |
|------------|--------|
| User auth (Supabase IdP, JWKS verification) | Chunk 1 — shipped |
| `/api/v1/...` aliases + 501 stubs + desktop wiring | Chunk 2 — shipped |
| SnapTrade brokerage integration | **Chunk 3 — current** |
| Stripe-backed subscription billing | Chunk 4 |
| Background broker sync + Redis-backed rate limit | Chunk 5 |
| End-to-end encrypted device sync | Chunk 6 |

The desktop client is open-source (AGPL-3.0). This backend is **not**.

## Prerequisites

| Tool | Version |
|------|---------|
| Rust | pinned in `rust-toolchain.toml` (≥ 1.95) |
| Postgres | 16+ (Docker compose ships 16-alpine locally) |
| sqlx-cli | installed via `cargo install sqlx-cli --no-default-features --features rustls,postgres` |
| Docker | 24+ (for compose-up + image build) |
| Fly CLI | for deploys |

## Local development quickstart

```bash
cp .env.example .env
# Edit .env: set SUPABASE_URL to your Supabase project URL.

make compose-up      # starts Postgres + Adminer (http://localhost:8081)
make migrate         # applies migrations
make run             # cargo run (foreground)
```

Or all in one shot:

```bash
make dev             # compose-up + migrate + run
```

Smoke check:

```bash
curl -i http://localhost:8080/health
curl -i http://localhost:8080/ready
```

## Environment variables

| Var | Required | Default | Description |
|-----|----------|---------|-------------|
| `APP_HOST` | no | `0.0.0.0` | Bind host |
| `APP_PORT` | no | `8080` | Bind port |
| `APP_ENV` | yes | `development` | `development` / `staging` / `production` / `test` |
| `LOG_LEVEL` | no | `info` | Tracing env-filter directive |
| `LOG_FORMAT` | no | `pretty` (dev) / `json` (prod) | Log output format |
| `DATABASE_URL` | **yes** | — | Postgres connection string |
| `DATABASE_MAX_CONNECTIONS` | no | `10` | Pool size |
| `SUPABASE_URL` | **yes** | — | Supabase project URL (`https://xxx.supabase.co`) |
| `SUPABASE_JWT_AUDIENCE` | no | `authenticated` | Expected `aud` claim |
| `SUPABASE_SERVICE_ROLE_KEY` | no | empty | Service-role JWT (Chunks 2-4 only) |
| `MIZAN_CORS_ALLOWED_ORIGINS` | yes (auth env) | empty | Comma-separated origin list |
| `RATE_LIMIT_PER_MINUTE` | no | `100` | Per-IP rate limit |
| `SENTRY_DSN` | no | empty | Empty disables Sentry |
| `SENTRY_ENVIRONMENT` | no | tracks `APP_ENV` | Sentry environment tag |
| `SENTRY_TRACES_SAMPLE_RATE` | no | `0.1` | 0.0 – 1.0 |
| `MIZAN_TEST_JWT_SECRET` | test only | empty | HS256 secret for test-mode tokens. Always ignored in production. |

## Architecture

```
┌─────────────────────┐
│  Mizan Desktop App  │  Tauri client (AGPL)
│  (samisayyed1/      │
│   mizan-4)          │
└──────────┬──────────┘
           │ HTTPS + Bearer JWT
           ▼
┌─────────────────────┐        ┌──────────────────┐
│   Mizan Connect     │  JWKS  │ Supabase (IdP)   │
│   (this repo)       │ ──────►│ auth.users       │
│   Rust + Axum       │        │ (passwords here) │
└──────┬──────────────┘        └──────────────────┘
       │ SQLx
       ▼
┌─────────────────────┐
│ Postgres (Supabase) │  users, subscriptions, broker_connections,
│                     │  sync_jobs, audit_log
└─────────────────────┘
```

Module map:

```
src/
├── main.rs / lib.rs          binary + library root
├── config.rs                 figment-loaded, validated at startup
├── error.rs                  AppError + ErrorCode + IntoResponse
├── state.rs                  AppState (config + pool + JWKS)
├── telemetry.rs              tracing + Sentry init
├── server.rs                 Axum router composition
├── shutdown.rs               SIGTERM/SIGINT handlers
├── health.rs                 /health, /ready
├── db/                       Pool + migration runner
├── auth/                     JWKS, Supabase JWT, extractor, upsert
├── users/                    /v1/me handlers + repository
├── audit/                    audit_log writer
└── middleware/               request_id, security_headers, timeout
```

## API endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/health` | public | Liveness — version + commit |
| `GET` | `/ready` | public | Readiness — DB + JWKS health (503 if not ready) |
| `GET` | `/v1/me` | bearer | Current user (legacy path) |
| `PATCH` | `/v1/me` | bearer | Update `display_name` |
| `GET` | `/api/v1/user/me` | bearer | Current user (desktop call site) |
| `GET` | `/api/v1/subscription/plans` | optional | Stripe plan catalog (Chunk 4 stub — 501) |
| `POST` | `/api/v1/sync/brokerage/login-portal` | bearer | Issue a SnapTrade Connection Portal URL (10/hr/user) |
| `GET` | `/api/v1/sync/snaptrade/callback` | **public, state-bound** | SnapTrade redirect target |
| `GET` | `/api/v1/sync/brokerage/connections` | bearer | Live broker authorizations |
| `GET` | `/api/v1/sync/brokerage/accounts` | bearer | Live broker accounts |
| `GET` | `/api/v1/sync/brokerage/accounts/:id/holdings` | bearer | Live positions |
| `GET` | `/api/v1/sync/brokerage/accounts/:id/activities` | bearer | Transactions, paginated (`?cursor=&limit=`) |
| `POST` | `/api/v1/sync/brokerage/connections/:id/refresh` | bearer | Force SnapTrade re-poll |
| `DELETE` | `/api/v1/sync/brokerage/connections/:id` | bearer | Disconnect + soft-delete (idempotent) |

Error responses follow:

```json
{ "error": { "code": "unauthorized", "message": "missing Authorization header", "request_id": "5b1d…" } }
```

## Testing

```bash
make test         # spins up ephemeral Postgres via testcontainers
```

Tests use the HS256 fallback path (`APP_ENV=test` + `MIZAN_TEST_JWT_SECRET`). Production builds reject HS256 tokens regardless of env, so the test fallback can never be exploited in a release artifact.

To smoke-test against your real Supabase:

```bash
# Mint a real session JWT in your Supabase dashboard or via supabase-js,
# then:
TOKEN="eyJhbGciOiJSUzI1NiIs…"
curl -i -H "Authorization: Bearer $TOKEN" http://localhost:8080/v1/me
```

## Deploy to Fly.io

First-time:

```bash
fly launch --no-deploy
fly secrets set DATABASE_URL=…
fly secrets set SUPABASE_URL=…
fly secrets set MIZAN_CORS_ALLOWED_ORIGINS=https://app.mizan.app
fly deploy
```

Subsequent:

```bash
make deploy
fly logs
fly status
```

Roll back:

```bash
fly releases
fly releases rollback <version>
```

See `.claude/skills/deploy-to-fly.md` for the runbook.

## Connecting from Mizan desktop

The desktop client (`samisayyed1/mizan-4`) reads `CONNECT_AUTH_URL` and friends at build time. Point them at Connect's URL once Chunk 2 lands and Stripe is wired up. Full client integration guide is part of Chunk 2.

## Roadmap

- **Chunk 1 (shipped):** foundation, JWT auth, `/v1/me`.
- **Chunk 2 (shipped):** `/api/v1/...` aliases + 501 stubs + desktop wiring.
- **Chunk 3 (current):** SnapTrade integration — broker connection lifecycle,
  HMAC-signed client, AES-256-GCM userSecret-at-rest, callback state JWTs,
  per-user rate limit.
- **Chunk 4:** Stripe billing — webhooks, subscription state machine, plan limits.
- **Chunk 5:** background broker sync (4-hour poll), dead-letter handling,
  Redis-backed rate limit (drops the in-memory `LoginPortalLimiter`).
- **Chunk 6:** E2EE device sync — paired devices, encrypted blobs, key rotation.

## License

All Rights Reserved. Proprietary. Not for distribution.
