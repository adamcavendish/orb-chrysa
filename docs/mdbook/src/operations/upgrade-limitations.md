# Upgrade Limitations

The current beta is the first production contract. Compatibility with earlier
local or development snapshots, config files, CLI bundle layouts, and chart
layouts is not guaranteed.

Known limitations:

- No snapshot format migration is provided for pre-beta snapshots.
- Old `[raft.tls].ca_path` config is not accepted; use `server_ca_path` and
  `client_ca_path`.
- Old air-gapped bundle layouts are not supported; regenerate bundles with the
  current CLI.
- Helm is the supported Kubernetes production install path.
- External S3-compatible storage is required for production.

For a pre-beta environment, export needed images, recreate the cluster with the
new chart and config schema, then push or mirror images into the new registry.
