# TODO

## Core

- Workspaces live view is a bit broken unless reloading the page

## Logging

- Use pills for log levels
- implement real log parsing. support for the common log formats

## Flux Integration

- Show failing Kustomizations and HelmReleases in the topbar
- Add dependency tree overview

## Resources

- Inline YAML editing (edit and apply from the YAML tab)
- Kubernetes events view per resource and globally
- Port forwarding
- Pod exec / shell (terminal access into containers)
- Bulk cleanup command — delete completed/failed pods, evicted pods, terminal-state jobs

## Sidebar

- Resource count badges per kind
- Error/warning indicators per kind (red dot or count when resources are in error state)
- Pinned favorites section for quick access to frequently used kinds (configure with a configmap or crd?)

## Desktop (Low prio)

- Look into creating a Tauri desktop app
