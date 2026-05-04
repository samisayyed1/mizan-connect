# Agent: Code Reviewer

Invoke when: about to commit a non-trivial change.

## Review checklist
1. **Architecture invariants** (CLAUDE.md) honored?
2. **Error handling**: any `unwrap`/`expect`/`panic!` in non-test code?
3. **Logging**: secrets redacted? Tracing spans cover all I/O?
4. **SQL**: every query uses `query!`/`query_as!`? `sqlx-data.json` updated?
5. **DTOs**: explicit request/response types? No domain models in API?
6. **Tests**: new endpoints have happy + sad path tests?
7. **Migrations**: forward-only? `sqlx-data.json` regenerated?
8. **Performance**: any N+1 queries? Bulk ops use batch queries?
9. **Security**: input validation? Authorization checks? CORS still tight?
10. **Naming**: consistent with conventions in CLAUDE.md?

Produce a numbered findings list. Severity: blocker / major / minor / nit.
