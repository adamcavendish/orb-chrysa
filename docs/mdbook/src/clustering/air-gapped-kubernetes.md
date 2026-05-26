# Air-Gapped Kubernetes

Orb Chrysa behaves like a normal OCI registry in Kubernetes. Workloads can pull
images from it after the node container runtime trusts the registry certificate.
`imagePullSecrets` only handle authentication; TLS trust is configured on every
node.

The beta production path is Helm-first:

1. Generate registry TLS and Raft mTLS certificates with the CLI.
2. Generate a Kubernetes bundle with TLS Secrets, containerd trust files, and a
   Helm values file.
3. Fill in external S3 settings in the generated values file.
4. Install the chart from `deploy/kubernetes/helm`.

## Generate Certificates

Use the CLI from an operator workstation:

```bash
orb-chrysa-cli air-gapped cert init \
  --registry-host registry.internal.example.com \
  --namespace orb-chrysa \
  --statefulset-name orb-chrysa \
  --headless-service orb-chrysa-headless \
  --replicas 3 \
  --out ./orb-chrysa-airgap
```

This creates an internal CA, a public registry TLS leaf certificate, and a Raft
mTLS leaf certificate whose SANs cover the StatefulSet pod DNS names. The Raft
certificate includes wildcard headless-service SANs so later scale-up ordinals
do not require immediate certificate rotation.

```text
orb-chrysa-airgap/
  certs/
    ca.crt
    ca.key
    server/
      tls.crt
      tls.key
    raft/
      ca.crt
      tls.crt
      tls.key
```

Keep `ca.key` offline. Do not mount CA private keys into Kubernetes.

## Generate Kubernetes, Helm, And containerd Files

```bash
IMAGE_TAG="replace-with-mirrored-server-tag"

orb-chrysa-cli air-gapped k8s bundle-generate \
  --registry-endpoint registry.internal.example.com:32000 \
  --cert-dir ./orb-chrysa-airgap/certs \
  --namespace orb-chrysa \
  --server-tls-secret orb-chrysa-server-tls \
  --raft-tls-secret orb-chrysa-raft-mtls \
  --image-repository registry.internal.example.com:32000/orb-chrysa-server \
  --image-tag "$IMAGE_TAG" \
  --out ./orb-chrysa-airgap
```

If `--image-repository` and `--image-tag` are omitted, the generated Helm values
default to GHCR and the installed `orb-chrysa-cli` package version. Set both when
operators mirror the server image into an internal registry.

The bundle contains:

```text
containerd/
  ca.crt
  hosts.toml
  install.md
helm/
  values-air-gapped.yaml
k8s/
  server-tls-secret.yaml
  raft-mtls-secret.yaml
  server-tls-config.toml
  raft-tls-config.toml
  image-pull-secret.example.yaml
README.md
```

The generated Kubernetes Secrets include the public registry certificate/key and
the Raft mTLS certificate/key/CA bundles. They do not include CA private keys.

## Install With Helm

Edit `orb-chrysa-airgap/helm/values-air-gapped.yaml` and set the external S3
endpoint, bucket, credentials Secret, and any public Service settings.
If the registry is exposed through a NodePort, load balancer, or external DNS
name, keep that hostname in `server.tls.dnsNames` so cert-manager-generated
public certificates include it.
For NodePort installs, set `service.type: NodePort` and `service.nodePort` in
the same values file.

Then install:

```bash
helm upgrade --install orb-chrysa ./deploy/kubernetes/helm \
  --namespace orb-chrysa \
  --create-namespace \
  -f ./orb-chrysa-airgap/helm/values-air-gapped.yaml
```

The chart creates a three-pod StatefulSet by default. The public registry/API
listener uses port `5050`; the internal Raft listener uses port `5051` and has
mutual TLS enabled by default.

## Configure Node Trust

Install the generated `ca.crt` and `hosts.toml` on every node through your normal
node-management channel: node image, cloud-init, Talos machine config, Flatcar
Ignition, OpenShift MachineConfig, or another platform-native mechanism.

For containerd, the target paths include the registry port:

```text
/etc/containerd/certs.d/registry.internal.example.com:32000/ca.crt
/etc/containerd/certs.d/registry.internal.example.com:32000/hosts.toml
```

Restart or reload containerd according to the node OS.

Orb Chrysa does not generate a privileged node-trust DaemonSet by default. If an
operator has permission to run a privileged DaemonSet that writes `/etc/containerd`,
they already have permission to mutate the host and should use the platform's
standard node-management path instead.

## Verify

From an operator workstation:

```bash
curl --cacert ./orb-chrysa-airgap/certs/ca.crt \
  https://registry.internal.example.com:32000/v2/
```

From a Kubernetes node:

```bash
crictl pull registry.internal.example.com:32000/qa/alpine:v1
```

From Kubernetes:

```bash
kubectl run orb-chrysa-pull-test \
  --image=registry.internal.example.com:32000/qa/alpine:v1 \
  --restart=Never
```

If authentication is enabled, create an image pull secret using the same endpoint,
including the port.

## Kubernetes Smoke

The repository includes an opt-in Helm smoke script that records evidence under
`/tmp/orb-k8s-<run_id>`:

```bash
REGISTRY_ENDPOINT=registry.internal.example.com:32000 \
NODE_TRUST_COMMAND='./install-node-trust.sh' \
tests/k8s/helm-smoke.sh
```

`NODE_TRUST_COMMAND` is intentionally operator-supplied because installing
containerd trust is node-platform specific. Set `DOCKER_TRUST_COMMAND` too if
the local Docker daemon used by the script does not already trust the generated
registry CA.
