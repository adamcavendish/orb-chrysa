# Kubernetes Deployment

orb-chrysa runs as a StatefulSet with automatic Raft membership management.

## Architecture

```
┌─────────────────────────────────────────────┐
│                  Kubernetes                  │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐     │
│  │ orb-0   │  │ orb-1   │  │ orb-2   │     │
│  │ Raft ✓  │  │ Raft ✓  │  │ Raft ✓  │     │
│  └─────────┘  └─────────┘  └─────────┘     │
│       │            │            │            │
│       └────────────┼────────────┘            │
│                    │                         │
│            ┌───────┴───────┐                 │
│            │  S3 (external) │                │
│            └───────────────┘                 │
└─────────────────────────────────────────────┘
```

- **StatefulSet** provides stable hostnames (`orb-chrysa-0`, `orb-chrysa-1`, ...)
- **DNS discovery** (`discovery_dns = "orb-chrysa"`) enables automatic peer discovery
- **Kubernetes reconciler** adjusts Raft membership when replicas change
- **No PVC** — Raft log uses ephemeral redb; state recovers from S3 snapshots

## Install

```bash
helm install orb-chrysa ./deploy/kubernetes/helm \
  --namespace orb-chrysa \
  --create-namespace \
  --set storage.s3.endpoint=https://s3.example.internal \
  --set storage.s3.bucket=orb-chrysa \
  --set storage.s3.region=us-east-1
```

See [values.yaml](helm/values.yaml) for all options.
