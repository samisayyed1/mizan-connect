# Skill: Deploy to Fly.io

Use when: deploying or updating the Fly.io app.

## First-time setup (already done if `fly.toml` exists)
```bash
fly launch --no-deploy            # generates fly.toml, app name
fly secrets set DATABASE_URL=... SUPABASE_URL=... SUPABASE_JWT_AUDIENCE=authenticated MIZAN_CORS_ALLOWED_ORIGINS=...
```

## Subsequent deploys
```bash
make deploy                       # = fly deploy
fly logs                          # tail logs
fly status                        # see machines + health
fly ssh console                   # debug
```

## Secrets management
- Never commit secrets to git.
- Set via `fly secrets set KEY=value`.
- Rotate quarterly. Document rotations in `docs/runbook.md` (create when needed).

## Rollback
```bash
fly releases                      # list
fly releases rollback <version>
```

## Health checks
- Fly's health check hits `/health` every 10s.
- A failed `/health` causes Fly to restart the machine.
- `/ready` is for human/operator checks (verifies DB + JWKS); not used for auto-restart.

## Region
- Primary region: `sin` (Singapore) — closest to current user base.
- Add regions later as users grow geographically.
