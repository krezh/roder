# TODO

## Talos Linux Integration

- Node service status (etcd, kubelet, apid health)
- dmesg / kernel log viewer
- Disk & mount usage
- Network interface state
- Per-node Talos version + machine config hash
- Machine config diff (show what diverged, not just the hash)
- Action: reboot / shutdown a node

## Nodes

- drain (prerequisite for reboot/shutdown actions)
- Node shell via privileged debug pod (nsenter) — Talos has no SSH

## Workloads

- Rollout history + rollback to revision, pause/resume
- Evict pod (PDB-respecting, distinct from delete)
- File copy to/from containers (kubectl cp equivalent over exec streams)
- Job re-run (recreate completed Job from spec)

## Helm

- Release revision history + rollback
- Computed values view
- Manifest diff between revisions

## Resources

- Port forwarding
- RBAC / access review (show what the current user can/can't do, given OIDC passthrough)
- Server-side dry-run diff before YAML apply
- Cluster-wide warning events timeline (sorted, deduped by count)

## Integrations

- Prometheus range queries — CPU/mem history graphs on pod/node detail
- cert-manager: surface Certificate expiry/renewal state + force renew action

## Desktop (Low prio)

- Look into creating a Tauri desktop app
