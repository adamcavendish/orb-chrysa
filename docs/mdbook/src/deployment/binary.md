# Binary

Run orb-chrysa directly from the binary. No root required, no container
runtime needed. Deploy with systemd or oxmgr for process supervision.

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

Copy the binary to each host. Set `HOSTNAME` to match the Raft node identity
convention (`<prefix>-<N>`):

```bash
# Host 1 (bootstraps cluster)
HOSTNAME=orb-chrysa-0 orb-chrysa-server --config deploy/binary/config/cluster.toml

# Host 2
HOSTNAME=orb-chrysa-1 orb-chrysa-server --config deploy/binary/config/cluster.toml

# Host 3
HOSTNAME=orb-chrysa-2 orb-chrysa-server --config deploy/binary/config/cluster.toml
```

Peers discover each other via DNS. Set up DNS records or `/etc/hosts` entries
for `orb-chrysa-0`, `orb-chrysa-1`, `orb-chrysa-2`.

## Process management

### oxmgr (recommended)

```bash
oxmgr apply deploy/binary/oxmgr/oxfile.toml
```

oxmgr provides restart-on-failure, log rotation, and health checks without
root privileges. See [OxMgr](https://github.com/Vladimir-Urik/OxMgr).

### systemd

```bash
sudo cp deploy/binary/systemd/orb-chrysa.service /etc/systemd/system/
sudo systemctl enable --now orb-chrysa
```

Systemd requires root for installation. The service runs as the `orb-chrysa`
user. Adjust `User=` and `ExecStart=` in the unit file for your environment.

## Configuration paths

Binary deployment uses paths relative to the working directory:

```
./config.toml          # orb-chrysa config (or set ORB_CHRYSA_CONFIG)
./data/raft/           # Raft log (ephemeral, no backup needed)
```

No `/etc/orb-chrysa/` writes required. Override with:

```bash
orb-chrysa-server --config /path/to/config.toml
```

## Upgrading

```bash
# 1. Graceful shutdown (uploads snapshot, leaves Raft)
kill -SIGTERM $(pidof orb-chrysa-server)

# 2. Replace binary
cp orb-chrysa-server-new /usr/local/bin/orb-chrysa-server

# 3. Restart
orb-chrysa-server --config config.toml
```

The node rejoins the Raft cluster automatically on restart. Snapshots
restored from S3 ensure no data loss.
