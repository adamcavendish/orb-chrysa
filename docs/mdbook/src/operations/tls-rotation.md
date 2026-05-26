# TLS Rotation

## Public Registry TLS

1. Issue a new certificate for the public registry endpoint.
2. Update the Secret referenced by `server.tls.existingSecret`.
3. Restart the StatefulSet pods one at a time:

   ```bash
   kubectl -n orb-chrysa rollout restart statefulset/orb-chrysa
   kubectl -n orb-chrysa rollout status statefulset/orb-chrysa
   ```

4. Update node container runtime trust if the issuing CA changed.

## Raft mTLS

Raft mTLS certs are loaded at process start. Rotate by updating the Secret
referenced by `raft.tls.existingSecret`, then rolling the StatefulSet.

If the Raft CA changes, use a staged CA bundle:

1. Add both old and new CA certificates to `server-ca.crt` and `client-ca.crt`.
2. Roll all pods so every peer trusts both CAs.
3. Replace the Raft leaf certificate/key with certs signed by the new CA.
4. Roll all pods again.
5. Remove the old CA from both bundles.
6. Roll all pods a final time.

This avoids splitting the cluster between peers that trust different CAs.
