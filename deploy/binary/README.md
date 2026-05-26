# Binary Deployment

Run orb-chrysa directly from the binary. No root required. Deploy from a
self-contained tarball or build from source.

## Tarball deployment (recommended)

```bash
# Build the tarball (includes orb-chrysa + RustFS + OxMgr)
just pack-binary

# Or with explicit versions
RUSTFS_URL=https://github.com/rustfs/rustfs/releases/download/v1.0.0-beta.6/rustfs-linux-x86_64-gnu-latest.zip \
OXMGR_URL=https://github.com/Vladimir-Urik/OxMgr/releases/download/v0.3.0/oxmgr-x86_64-unknown-linux-gnu.tar.gz \
  just pack-binary
```

The tarball contains:
```
orb-chrysa-0.1.0-x86_64-unknown-linux-gnu.tar.gz
  bin/
    orb-chrysa-server     # the registry
    rustfs                # S3-compatible storage
    oxmgr                 # process manager
  config/
    standalone.toml       # ready-to-run single-node config
  oxfile.toml             # oxmgr process group
  README                  # quick-start instructions
```

Extract and run:
```bash
tar xzf orb-chrysa-0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd orb-chrysa-*

# Option A: oxmgr (all-in-one)
./bin/oxmgr apply oxfile.toml

# Option B: manual
./bin/rustfs &
./bin/orb-chrysa-server --config config/standalone.toml
```

The tarball is self-contained — no external downloads, no root access needed.
Copy it to any Linux x86_64 host and run.

## Prerequisites

- orb-chrysa binary on `$PATH` (or use absolute path)
- RustFS running (binary or container)
- S3-compatible bucket created in RustFS

## Quick start (single node)

```bash
# 1. Start RustFS
rustfs &
# 2. Create bucket (one-time)
rc alias set local http://127.0.0.1:9000 mykey mysecret
rc bucket create local/orb-chrysa -p
# 3. Start orb-chrysa
orb-chrysa-server --config deploy/binary/config/standalone.toml
```

## Cluster (3 nodes)

On each host, set `HOSTNAME` and start:

```bash
# Host 1
HOSTNAME=orb-chrysa-0 orb-chrysa-server --config deploy/binary/config/cluster.toml

# Host 2
HOSTNAME=orb-chrysa-1 orb-chrysa-server --config deploy/binary/config/cluster.toml

# Host 3
HOSTNAME=orb-chrysa-2 orb-chrysa-server --config deploy/binary/config/cluster.toml
```

Each node discovers peers via DNS (`discovery_dns` in config).

## Process management

### oxmgr (recommended)

```bash
oxmgr apply deploy/binary/oxmgr/oxfile.toml
```

### systemd

```bash
sudo cp deploy/binary/systemd/orb-chrysa.service /etc/systemd/system/
sudo systemctl enable --now orb-chrysa
```

Systemd requires root for installation. The service runs as the `orb-chrysa` user.

## Configuration paths

Binary deployment uses paths relative to the working directory by default:

```
./config.toml          # orb-chrysa config
./data/raft/           # Raft log (ephemeral)
```

No `/etc/orb-chrysa/` writes required. Override with environment variables:
```bash
ORB_CHRYSA_CONFIG=/path/to/config.toml orb-chrysa-server
```
