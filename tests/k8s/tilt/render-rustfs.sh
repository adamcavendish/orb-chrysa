#!/usr/bin/env bash
set -euo pipefail

NAMESPACE="${RUSTFS_NAMESPACE:-orb-chrysa-tilt-s3}"
BUCKET="${S3_BUCKET:-orb-chrysa}"
ACCESS_KEY="${S3_ACCESS_KEY:-rustfsadmin}"
SECRET_KEY="${S3_SECRET_KEY:-rustfsadmin}"
RUSTFS_IMAGE="${RUSTFS_IMAGE:-rustfs/rustfs:1.0.0-beta.2}"
RUSTFS_RC_IMAGE="${RUSTFS_RC_IMAGE:-rustfs/rc:latest}"

cat <<YAML
apiVersion: v1
kind: Namespace
metadata:
  name: $NAMESPACE
---
apiVersion: v1
kind: Secret
metadata:
  name: rustfs-root
  namespace: $NAMESPACE
type: Opaque
stringData:
  access_key: "$ACCESS_KEY"
  secret_key: "$SECRET_KEY"
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: rustfs
  namespace: $NAMESPACE
  labels:
    app: rustfs
spec:
  replicas: 1
  selector:
    matchLabels:
      app: rustfs
  template:
    metadata:
      labels:
        app: rustfs
    spec:
      containers:
        - name: rustfs
          image: $RUSTFS_IMAGE
          imagePullPolicy: IfNotPresent
          env:
            - name: RUSTFS_ACCESS_KEY
              valueFrom:
                secretKeyRef:
                  name: rustfs-root
                  key: access_key
            - name: RUSTFS_SECRET_KEY
              valueFrom:
                secretKeyRef:
                  name: rustfs-root
                  key: secret_key
            - name: RUSTFS_ADDRESS
              value: "0.0.0.0:9000"
            - name: RUSTFS_CONSOLE_ADDRESS
              value: "0.0.0.0:9001"
            - name: RUSTFS_VOLUMES
              value: /data
          ports:
            - name: api
              containerPort: 9000
            - name: console
              containerPort: 9001
          readinessProbe:
            httpGet:
              path: /health
              port: api
            initialDelaySeconds: 5
            periodSeconds: 5
          volumeMounts:
            - name: data
              mountPath: /data
      volumes:
        - name: data
          emptyDir: {}
---
apiVersion: v1
kind: Service
metadata:
  name: rustfs
  namespace: $NAMESPACE
spec:
  selector:
    app: rustfs
  ports:
    - name: api
      port: 9000
      targetPort: api
    - name: console
      port: 9001
      targetPort: console
---
apiVersion: batch/v1
kind: Job
metadata:
  name: rustfs-init
  namespace: $NAMESPACE
spec:
  manualSelector: true
  selector:
    matchLabels:
      job-name: rustfs-init
  template:
    metadata:
      labels:
        job-name: rustfs-init
    spec:
      restartPolicy: Never
      containers:
        - name: rc
          image: $RUSTFS_RC_IMAGE
          imagePullPolicy: IfNotPresent
          command: ["/bin/sh", "-ec"]
          args:
            - |
              RUSTFS_IP=\$(getent hosts rustfs.$NAMESPACE.svc.cluster.local | awk '{print \$1}')
              rc alias set local http://\${RUSTFS_IP}:9000 "$ACCESS_KEY" "$SECRET_KEY"
              rc bucket create local/$BUCKET -p
  backoffLimit: 3
YAML
