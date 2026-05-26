# Kubernetes Test Harnesses

This directory contains Kubernetes smoke tests and helpers.

| Path | Purpose |
|---|---|
| `helm-smoke.sh` | Opt-in Helm smoke harness for real or local clusters. |
| `tilt-*-smoke.sh` | Tilt/kind production-like smoke, failure, recovery, and scale tests. |
| `lib.sh` | Shared shell helpers for Kubernetes smoke scripts. |
| `tilt/` | Tilt-owned kind cluster setup and rendered fixture resources. |

These files are test harnesses, not production install artifacts. The production
Kubernetes artifact is `deploy/kubernetes/helm`.
