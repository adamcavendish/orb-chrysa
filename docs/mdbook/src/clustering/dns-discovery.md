# DNS Discovery

Orb Chrysa uses DNS-based peer discovery instead of a static peer list. All nodes
resolve the configured `discovery_dns` name and discover peers dynamically.

## How It Works

1. On startup, each node performs a DNS lookup of `discovery_dns`
2. For each resolved IP, it probes `/raft/status` over HTTP or HTTPS,
   depending on whether `[raft.tls]` is enabled
3. If a reachable cluster exists, it joins via `POST /raft/join`
4. If no cluster exists and the node has ordinal 0, it bootstraps

## Docker Compose

```yaml
services:
  orb-chrysa-0:
    hostname: orb-chrysa-0
    networks:
      default:
        aliases:
          - orb-chrysa
  orb-chrysa-1:
    hostname: orb-chrysa-1
    networks:
      default:
        aliases:
          - orb-chrysa
```

The `orb-chrysa` network alias resolves to all nodes' IPs. `discovery_dns = "orb-chrysa"`
uses Docker's built-in DNS.

## Kubernetes

```yaml
apiVersion: v1
kind: Service
metadata:
  name: orb-chrysa
spec:
  clusterIP: None
  selector:
    app: orb-chrysa
  ports:
  - port: 5051
    name: raft
```

The headless service `orb-chrysa` returns all pod IPs. `discovery_dns = "orb-chrysa"`
uses Kubernetes DNS.

## Node Identity

Node ID is derived from the hostname suffix:

| Hostname | Ordinal | Node ID |
|----------|---------|---------|
| `orb-chrysa-0` | 0 | 1 |
| `orb-chrysa-1` | 1 | 2 |
| `orb-chrysa-2` | 2 | 3 |

Ordinal 0 bootstraps the cluster if no existing cluster is found.
