#!/usr/bin/env bash
set -euo pipefail

NAMESPACE="${KANIDM_NAMESPACE:-kanidm}"
IMAGE="${KANIDM_IMAGE:-kanidm/server:1.10.3}"
NODE_PORT="${KANIDM_NODE_PORT:-30443}"
HOST_PORT="${KANIDM_HOST_PORT:-8443}"

cat <<YAML
apiVersion: v1
kind: Namespace
metadata:
  name: $NAMESPACE
---
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: kanidm-tls
  namespace: $NAMESPACE
spec:
  secretName: kanidm-tls
  issuerRef:
    name: orb-chrysa-ca
    kind: ClusterIssuer
    group: cert-manager.io
  dnsNames:
    - localhost
    - kanidm
    - kanidm.$NAMESPACE
    - kanidm.$NAMESPACE.svc
    - kanidm.$NAMESPACE.svc.cluster.local
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: kanidm-data
  namespace: $NAMESPACE
spec:
  accessModes:
    - ReadWriteOnce
  resources:
    requests:
      storage: 1Gi
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: kanidm-config
  namespace: $NAMESPACE
data:
  server.toml: |
    bindaddress = "0.0.0.0:8443"
    db_path = "/data/kanidm.db"
    tls_chain = "/certs/tls.crt"
    tls_key = "/certs/tls.key"
    domain = "localhost"
    origin = "https://localhost:$HOST_PORT"
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: kanidm
  namespace: $NAMESPACE
  labels:
    app: kanidm
spec:
  replicas: 1
  selector:
    matchLabels:
      app: kanidm
  template:
    metadata:
      labels:
        app: kanidm
    spec:
      containers:
        - name: kanidm
          image: $IMAGE
          imagePullPolicy: IfNotPresent
          ports:
            - name: https
              containerPort: 8443
          readinessProbe:
            exec:
              command: ["kanidmd", "scripting", "healthcheck"]
            initialDelaySeconds: 10
            periodSeconds: 5
            failureThreshold: 12
          volumeMounts:
            - name: data
              mountPath: /data
            - name: config
              mountPath: /data/server.toml
              subPath: server.toml
              readOnly: true
            - name: tls
              mountPath: /certs
              readOnly: true
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: kanidm-data
        - name: config
          configMap:
            name: kanidm-config
        - name: tls
          secret:
            secretName: kanidm-tls
---
apiVersion: v1
kind: Service
metadata:
  name: kanidm
  namespace: $NAMESPACE
spec:
  type: NodePort
  selector:
    app: kanidm
  ports:
    - name: https
      port: 8443
      targetPort: https
      nodePort: $NODE_PORT
YAML
