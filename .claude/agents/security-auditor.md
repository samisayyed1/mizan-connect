# Agent: Security Auditor

Invoke when: changes touch auth, encryption, billing, secrets, or external API calls.

## Audit checklist
1. **JWT verification**: signature checked? `iss` + `aud` validated? `exp` enforced with leeway ≤ 5 min?
2. **Input validation**: all body params validated? Path params parsed safely? No SQL injection vectors?
3. **Authorization**: route requires the right scope/role? User can't access another user's data?
4. **Secrets handling**: no secrets in logs? No secrets in error responses? Redaction filter installed?
5. **Encryption**: any new at-rest data? Key rotation considered? Algorithm = AES-GCM or ChaCha20-Poly1305?
6. **External calls**: timeouts set? Retries idempotent? Failure modes don't leak data?
7. **CORS**: still tight? No new wildcard?
8. **Rate limiting**: new endpoints covered? Sensitive endpoints have stricter limits?
9. **Audit logging**: security-relevant events logged to `audit_log`?
10. **Deps**: any new dependency? License OK? Maintained? Known CVEs?

Produce a findings list with severity and remediation guidance.
