# Raft Configuration

```toml
[raft]
listen = "0.0.0.0:5051"
data_dir = "/tmp/raft"
discovery_dns = "orb-chrysa"

[raft.tls]
cert_path = "/certs/cert.pem"
key_path = "/certs/key.pem"
server_ca_path = "/certs/ca.pem"
client_ca_path = "/certs/ca.pem"
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `listen` | string | `"0.0.0.0:5051"` | Address for Raft RPC (vote, append, snapshot) |
| `data_dir` | string | `"/tmp/raft"` | Directory for ephemeral redb log |
| `discovery_dns` | string | (required) | DNS name for peer discovery |
| `tls` | optional | `None` | Mutual TLS configuration for Raft RPC |
| `tls.cert_path` | string | (required if tls set) | Peer certificate path. The same certificate is presented to other peers. |
| `tls.key_path` | string | (required if tls set) | Peer private key path |
| `tls.server_ca_path` | string | (required if tls set) | CA bundle used by the Raft client to verify peer server certificates |
| `tls.client_ca_path` | string | (required if tls set) | CA bundle used by the Raft server to verify peer client certificates |

## DNS Discovery

`discovery_dns` is the DNS name used to discover peer nodes. All DNS records
matching this name are treated as potential peers. In Docker Compose, this is
a network alias. In Kubernetes, this is the headless service name.

## TLS

When `tls` is configured, all Raft RPC traffic is encrypted with mutual TLS.
Every Raft server requires a client certificate signed by `client_ca_path`, and
every Raft client validates peer server certificates against `server_ca_path`.
For a shared internal CA, set both CA paths to the same file.

Helm clustered deployments enable Raft mTLS by default. The public registry/API
listener uses `[server.tls]` and is intentionally configured separately from
Raft mTLS.
