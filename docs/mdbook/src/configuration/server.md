# Server Configuration

```toml
[server]
listen = "0.0.0.0:5050"

[server.limits]
max_concurrent_uploads = 64
max_concurrent_requests = 512

[server.tls]
cert_path = "/etc/orb-chrysa/tls/tls.crt"
key_path = "/etc/orb-chrysa/tls/tls.key"
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `listen` | string | `"0.0.0.0:5050"` | Address and port for the HTTP API |
| `limits.max_concurrent_uploads` | integer | 64 | Maximum simultaneous blob uploads |
| `limits.max_concurrent_requests` | integer | 512 | Maximum concurrent HTTP requests |
| `tls` | optional | `None` | Native HTTPS configuration for the public registry/API listener |
| `tls.cert_path` | string | required if `tls` set | PEM certificate chain path |
| `tls.key_path` | string | required if `tls` set | PEM private key path |

The public server listener handles OCI Distribution API traffic, dashboard API
traffic, OAuth2 endpoints, metrics, health checks, and the dashboard SPA.

When `[server.tls]` is configured, that public listener serves HTTPS directly.
Kubernetes image pulls still require every node's container runtime to trust the
issuing CA.

Raft always uses a separate peer-to-peer listener configured by `[raft].listen`.
`[server].listen` and `[raft].listen` must bind different addresses. Production
Helm clustered deployments enable `[raft.tls]` and require mutual TLS for Raft
traffic.
