# TODO

## Talos Linux Integration

- [ x ] Initial in-cluster integration: node version, service health, mounts, and network counters
- [x] Node service status actions and richer health details
- [x] dmesg / kernel log viewer
- Disk inventory
- [x] Disk I/O statistics
- Network interface addresses and link state
- Per-node Talos version + machine config hash
- Machine config diff (show what diverged, not just the hash)
- [x] Action: reboot / shutdown a node

## Workloads

- Rollout history + rollback to revision, pause/resume
- File copy to/from containers (kubectl cp equivalent over exec streams)
- Job re-run (recreate completed Job from spec)

## Helm

- Release revision history + rollback
- Computed values view
- Manifest diff between revisions

## Resources

- Port forwarding
- Server-side dry-run diff before YAML apply
- Cluster-wide warning events timeline (sorted, deduped by count)

## Integrations

- Prometheus range queries — CPU/mem history graphs on pod/node detail
- cert-manager: surface Certificate expiry/renewal state + force renew action

## Mobile

- Improve mobile GUI (touch target sizing, reflow desktop-shared components for narrow screens)

## Desktop (Low prio)

- Look into creating a Tauri desktop app
