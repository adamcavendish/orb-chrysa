# Quick Start

Get orb-chrysa running locally in under a minute.

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/) and Docker Compose
- [ORAS](https://oras.land/) (optional, for OCI artifact tests)
- [just](https://github.com/casey/just) (optional, for convenience commands)

## Start the Cluster

```bash
# 3-node cluster with local S3 (RustFS)
docker compose -f deploy/compose/cluster.yml up -d
```

Wait for all services to be healthy:

```bash
docker compose -f deploy/compose/cluster.yml ps
# All orb-chrysa-N services should show "healthy"
```

Check cluster status:

```bash
just cluster-status
# {"leader_id": 1, "quorum": 2, "healthy_voters": 3}
```

## Push and Pull

```bash
# Pull a small test image
docker pull alpine:latest

# Tag it for orb-chrysa
docker tag alpine:latest localhost:5050/hello-world/alpine:v1

# Push
docker push localhost:5050/hello-world/alpine:v1

# Remove local copy
docker rmi localhost:5050/hello-world/alpine:v1

# Pull from orb-chrysa
docker pull localhost:5050/hello-world/alpine:v1
```

## Using ORAS

```bash
echo "hello orb-chrysa" > artifact.txt
oras push --plain-http localhost:5050/hello-world/artifact:v1 artifact.txt
oras pull --plain-http localhost:5050/hello-world/artifact:v1
```

## Dashboard

Open [http://localhost:5050](http://localhost:5050) to browse repositories, manifests,
and cluster status.

## Auth-Enabled Cluster

```bash
# Start the auth cluster with kanidm
docker compose -f deploy/compose/auth-cluster.yml up -d

# docker login with a PAT (generated via dashboard or API)
echo "<your-pat>" | docker login localhost:5050 --username developer --password-stdin

# Push and pull as usual — auth is enforced
docker push localhost:5050/dev/my-app:v1

# Clean up
just compose-auth-down
```

See the [Authentication](authentication.md) section for details on setting up kanidm
and managing tokens.

## Next Steps

- [Architecture overview](architecture.md) — data flow and design
- [Configuration reference](reference/config-reference.md) — all config options
- [Clustering guide](clustering.md) — DNS discovery and snapshots
- [Test plans](../test-plans/) — production OCI workflow tests
