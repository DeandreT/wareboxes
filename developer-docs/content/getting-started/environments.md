# Environments

Wareboxes environment URLs and credentials are supplied during onboarding. Keep
credentials, idempotency keys, and test documents isolated by environment.

| Environment | Intended use | Data expectations |
| --- | --- | --- |
| Local development | Developer testing against a local server | Disposable |
| Sandbox | Contract and partner acceptance testing | Synthetic, non-production |
| Production | Live warehouse operations | Controlled operational data |

The generated reference uses `http://127.0.0.1:8080` for local development until
hosted sandbox and production API domains are finalized. Production integrations
must use TLS.

Do not copy production idempotency keys into sandbox. Idempotency identities are
part of an environment's operational history and should be generated independently.
