#!/usr/bin/env bash
set -euo pipefail

cat <<'YAML'
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: orb-chrysa-selfsigned
spec:
  selfSigned: {}
---
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: orb-chrysa-ca
  namespace: cert-manager
spec:
  isCA: true
  commonName: Orb Chrysa Tilt CA
  secretName: orb-chrysa-ca-secret
  privateKey:
    algorithm: ECDSA
    size: 256
  issuerRef:
    name: orb-chrysa-selfsigned
    kind: ClusterIssuer
    group: cert-manager.io
---
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: orb-chrysa-ca
spec:
  ca:
    secretName: orb-chrysa-ca-secret
YAML
