# OCI Conformance

This directory contains the local harness for the OCI Distribution conformance
suite.

## Tracked Files

- `run.sh` starts the standalone compose fixture and runs the upstream
  conformance binary against `http://localhost:5050`.
- `Dockerfile` is an optional wrapper for running a prebuilt conformance binary
  in a container.

## Generated Files

These files are local cache/evidence artifacts and are intentionally ignored:

- `conformance.test` - compiled upstream conformance binary.
- `.distribution-spec/` - shallow clone of the upstream distribution-spec repo.
- `results/` - conformance logs and reports.

By default the harness builds from the upstream `opencontainers/distribution-spec`
tag recorded in `distribution-spec.ref`. Override with `OCI_DISTRIBUTION_SPEC_REF`
only when intentionally refreshing conformance evidence against a newer upstream
release.

```bash
just conformance
```
