# Deployment

orb-chrysa supports three deployment modes:

| Mode | Directory | Best for |
|---|---|---|
| Kubernetes | [`kubernetes/`](kubernetes/) | Production clusters with StatefulSet auto-scaling |
| Docker Compose | [`compose/`](compose/) | Multi-replica deployments with rolling updates |
| Binary | [`binary/`](binary/) | Single-host or manual multi-host with systemd/oxmgr |

All three modes use the same orb-chrysa binary and configuration schema.
External S3-compatible storage is required in all modes.

## Quick reference

```bash
# Kubernetes (Helm)
helm install orb-chrysa ./deploy/kubernetes/helm -n orb-chrysa --create-namespace

# Docker Compose (3-node cluster)
docker compose -f deploy/compose/cluster.yml up -d

# Binary (single node)
orb-chrysa-server --config deploy/binary/config/standalone.toml
```
