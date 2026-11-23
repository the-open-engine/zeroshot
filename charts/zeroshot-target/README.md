# Zeroshot target Helm chart

The chart needs a Linux amd64 node and a writable volume that
supports POSIX ownership and permissions. The target starts as root to launch isolated worker UIDs,
so the namespace must allow root containers.
If you change the release name, use lowercase letters, digits, and hyphens, starting with a letter.

```sh
helm upgrade --install zeroshot-target \
  oci://ghcr.io/the-open-engine/charts/zeroshot-target \
  --namespace zeroshot --create-namespace --wait
kubectl --namespace zeroshot port-forward service/zeroshot-target 8080:8080
```

Helm selects the newest stable release. To pin the chart and its matching image, pass
`--version X.Y.Z` on install or upgrade. To try the source chart, use
`./charts/zeroshot-target` from the repository root in place of the OCI URL.

Use a Zeroshot CLI release that matches the target image. In a second terminal, register and
check the target. The UI at `http://127.0.0.1:8080/ui/` shows profiles and run history. For a
different local port, set `publicOrigin` to the matching loopback URL.

```sh
zeroshot target add kubernetes --url http://127.0.0.1:8080 --direct
zeroshot list --target kubernetes
```

Pass `--target kubernetes` to run and observation commands. See
[source and credentials](../../docs/concepts/targets.md#source-and-delivery-credentials).

Any workload that can reach the ClusterIP Service can submit runs, even when you access it through
port forwarding. Use a trusted cluster or restrict access with network policy. For an authenticated
proxy, restrict direct access to the Service, set `publicOrigin` to the client-facing HTTPS origin,
preserve the client-facing `Host` header, and forward WebSocket upgrades on `/native-v2/oecp`.

Set `image.tag`, `resources`, `storage.size`, and `storage.storageClassName` as needed in a values
file and pass it with `-f values.yaml`. Quote image tags. The chart pins the default image to its
`appVersion` and uses the cluster's default storage class. An existing claim's storage class is
fixed. Its size can increase if the storage class supports expansion.

Upgrades that replace the pod mark interrupted runs as lost.
Use `zeroshot resume RUN_ID --target kubernetes` when a checkpoint is available. The volume preserves
profiles and run history, and Helm retains the claim when uninstalling. Reinstall under the same
release name and namespace with the original storage values to reuse it. Use
`helm uninstall zeroshot-target --namespace zeroshot --cascade foreground --wait` to wait for the
old pod to stop before reinstalling.

The image includes Docker CLI, Buildx, and Compose. Operators provide a daemon reachable by the
run's worker UID. Compose bind mounts require paths that the daemon can access at the same absolute
location. See [Docker access](../../docker/zeroshot-target/README.md#environment-scripts-and-docker).
