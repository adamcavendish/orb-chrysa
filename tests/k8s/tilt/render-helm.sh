#!/usr/bin/env bash
set -euo pipefail

NAMESPACE="${ORB_NAMESPACE:-orb-chrysa-tilt}"
S3_NAMESPACE="${RUSTFS_NAMESPACE:-orb-chrysa-tilt-s3}"
RELEASE="${RELEASE:-orb-chrysa}"
CHART="${CHART:-deploy/kubernetes/helm}"
REGISTRY_ENDPOINT="${REGISTRY_ENDPOINT:-localhost:32050}"
KANIDM_HOST_PORT="${KANIDM_HOST_PORT:-8443}"
S3_BUCKET="${S3_BUCKET:-orb-chrysa}"
S3_ACCESS_KEY="${S3_ACCESS_KEY:-rustfsadmin}"
S3_SECRET_KEY="${S3_SECRET_KEY:-rustfsadmin}"
WORK="${WORK:-target/tilt/helm}"

mkdir -p "$WORK"
VALUES="$WORK/values.yaml"
cat > "$VALUES" <<YAML
replicaCount: 3

image:
  repository: orb-chrysa-server
  tag: tilt
  pullPolicy: IfNotPresent

service:
  type: NodePort
  nodePort: 32050

storage:
  s3:
    endpoint: http://rustfs.$S3_NAMESPACE.svc.cluster.local:9000
    bucket: $S3_BUCKET
    region: us-east-1
    pathStyle: true
    existingSecret: orb-chrysa-s3

server:
  tls:
    existingSecret: orb-chrysa-server-tls
    dnsNames:
      - localhost

raft:
  tls:
    existingSecret: orb-chrysa-raft-mtls

auth:
  enabled: true
  issuerUrl: https://localhost:$KANIDM_HOST_PORT/oauth2/openid/orb-chrysa
  issuerInternalUrl: https://kanidm.kanidm.svc.cluster.local:8443/oauth2/openid/orb-chrysa
  issuerInternalUrls:
    - https://kanidm.kanidm.svc.cluster.local:8443/oauth2/openid/orb-chrysa
  clientId: orb-chrysa
  tokenEndpointUrl: https://$REGISTRY_ENDPOINT/v2/token
  redirectUri: https://$REGISTRY_ENDPOINT/oauth2/callback
  tlsInsecureSkipVerify: true
  existingSecret: orb-chrysa-auth
  permissions:
    - name: admin-full-access
      groups: ["registry_admins"]
      scopes: ["repository:*:*"]
    - name: developer-access
      groups: ["registry_developers"]
      scopes: ["repository:dev/*:push", "repository:dev/*:pull"]

certManager:
  enabled: true
  issuerRef:
    name: orb-chrysa-ca
    kind: ClusterIssuer
    group: cert-manager.io
YAML

cat <<YAML
apiVersion: v1
kind: Namespace
metadata:
  name: $NAMESPACE
---
apiVersion: v1
kind: Secret
metadata:
  name: orb-chrysa-s3
  namespace: $NAMESPACE
type: Opaque
stringData:
  access_key: "$S3_ACCESS_KEY"
  secret_key: "$S3_SECRET_KEY"
---
YAML

helm template "$RELEASE" "$CHART" --namespace "$NAMESPACE" -f "$VALUES"
