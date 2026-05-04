# Skill: Add a SQLx query

Use when: writing or modifying any database query.

## Rules
1. Always use the macro form: `sqlx::query!`, `sqlx::query_as!`, `sqlx::query_scalar!`.
   - These check the SQL against the database at compile time.
2. After adding/changing any query, run:
   ```bash
   cargo sqlx prepare
   ```
3. Commit `sqlx-data.json` along with your code change.
4. The query macros require `DATABASE_URL` set during compilation OR `sqlx-data.json` (offline mode for CI).

## Patterns

### Fetch one (typed)
```rust
let user = sqlx::query_as!(
    User,
    r#"SELECT id, supabase_user_id, email, display_name, avatar_url,
              created_at, updated_at
       FROM users WHERE id = $1 AND deleted_at IS NULL"#,
    user_id
)
.fetch_optional(&pool)
.await?;
```

### Insert returning
```rust
let id = sqlx::query_scalar!(
    "INSERT INTO users (supabase_user_id, email) VALUES ($1, $2) RETURNING id",
    sub, email
).fetch_one(&pool).await?;
```

### Upsert
Use `ON CONFLICT (column) DO UPDATE SET ...`. The user upsert from JWT is the canonical example — see `src/auth/upsert.rs`.

## Don'ts
- Don't string-concat SQL. Use `$1, $2` parameters.
- Don't use `query!` (untyped) — prefer `query_as!` for selects.
- Don't `unwrap()` query results. Always propagate `?`.
