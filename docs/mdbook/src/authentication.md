# Authentication

orb-chrysa supports authentication via [kanidm](https://kanidm.com), a Rust identity
management platform. When the `[auth]` section is present in the config, all OCI and
dashboard API endpoints require authentication.

When `[auth]` is not configured, all endpoints remain open — the default for
development and evaluation.

## Architecture

```mermaid
graph LR
    Docker[Docker CLI] --> TokenEp[/v2/token]
    Browser[Dashboard] --> OIDC[/oauth2/*]
    CI[CI Pipeline] --> TokenEp
    TokenEp --> Auth[Auth Middleware]
    OIDC --> Auth
    Auth --> Kanidm[kanidm IdP]
    Kanidm --> JWKS[JWKS Cache]
    Auth --> JWKS
```

kanidm issues JWTs; orb-chrysa validates them locally via a cached JWKS endpoint.
No per-request calls to kanidm.

## Token Types

| Type | Issuer | Use case |
|------|--------|----------|
| **Personal Access Token** (PAT) | orb-chrysa | `docker login` for human users |
| **OCI Bearer Token** | orb-chrysa | Short-lived token from `/v2/token` |
| **kanidm Access Token** | kanidm | CI pipeline service accounts |

## Topics

- [Kanidm Setup](authentication/kanidm.md) — deploying and configuring kanidm
- [Personal Access Tokens](authentication/pat.md) — creating and managing PATs
- [CI / Service Accounts](authentication/service-accounts.md) — machine-to-machine auth
- [Dashboard OIDC](authentication/oidc.md) — browser-based login flow
