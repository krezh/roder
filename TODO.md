# TODO

## Correctness Foundations

- Centralize GVK-aware resource status calculation across lists, trees, dashboard rollups, and alert targets
- Carry canonical resource keys in health rollups instead of selecting resources by Kind
- Track unknown and unreadable resources in dashboard health instead of silently treating them as healthy
- Surface per-kind list and RBAC failures in dashboard rollups
- Map Warning and Normal Event types to meaningful row status
- Detect pending LoadBalancer Services and Ingresses
- Detect Endpoints and EndpointSlices without ready backends
- Treat intentionally scaled-to-zero workloads as healthy
- Evaluate workload rollout failure, observed generation, revision convergence, and availability conditions
- Derive HPA health from AbleToScale, ScalingActive, and ScalingLimited conditions
- Derive PDB health from conditions and desired/current healthy counts
- Treat Lost PVCs as errors
- Use Done for successfully completed resources where applicable
- Expand generic status support for common healthy, progressing, degraded, paused, completed, and failed phases
- Ignore stale conditions whose observed generation does not match the resource generation

## Actions And Access

- Introduce a central GVK capability registry shared by desktop, mobile, bulk actions, and backend validation
- Split Flux capabilities into reconcile, suspend, source reconciliation, force, and reset
- Restrict Flux actions to supported exact resource kinds and validate them server-side
- Restrict External Secrets refresh to supported resource kinds and validate it server-side
- Return attempted, succeeded, forbidden, and failed counts from reconcile-all operations
- Add batch action capability checks and show how many selected resources permit an operation
- Hide or disable context-menu, action-sheet, and bulk actions when RBAC denies them
- Model semantic operation permissions, including dependent resource and subresource checks
- Check Job create permission before triggering a CronJob
- Check Snapshot create permission before triggering a Kopiur SnapshotPolicy
- Check source and target permissions for Flux reconcile-with-source
- Pass no namespace to access reviews for cluster-scoped resources
- Add watch, status update, logs, exec, eviction, and dependent-resource operations to Access Review
- Group Access Review rows by category or operator

## Dashboard

- Replace hard-coded controller fields with generic controller health groups
- Add cert-manager health rollups
- Add Rook health rollups using Ceph health semantics
- Add CloudNativePG cluster, archival, and backup freshness rollups
- Show backup age as an RPO signal
- Add Prometheus and Alertmanager readiness rollups
- Ensure unknown resources cannot render with an OK card style

## Resource Relationships

- Introduce a GVK-keyed relationship provider registry
- Add Pod to PVC to PV to StorageClass relationships
- Add VolumeAttachment to PV and Node relationships
- Link Rook-backed StorageClasses to CephBlockPools and CephClusters
- Add CephCluster to pool, filesystem, object-store, and operator workload relationships
- Add ObjectBucketClaim to ObjectBucket, Secret, and ConfigMap relationships
- Add CloudNativePG Cluster to Pods, PVCs, Services, Backups, Databases, DatabaseRoles, and Poolers
- Add CloudNativePG ScheduledBackup to generated Backup relationships
- Link CloudNativePG Backup, Database, DatabaseRole, and Pooler resources to their Cluster
- Add cert-manager Certificate to target Secret and issuer relationships
- Add cert-manager CertificateRequest to issuer, Order, and Challenge relationships
- Add ExternalSecret to store and generated Secret relationships
- Add ClusterExternalSecret to generated ExternalSecret relationships
- Add Kopiur Policy to Schedule, Snapshot, Repository, and Restore relationships
- Add Tuppr upgrades to affected Nodes
- Add Flux sourceRef, chartRef, and dependsOn relationships
- Add Gateway API Route to parent Gateway and backend Service relationships
- Add Envoy policy to target Route, Gateway, or Service relationships
- Add ReferenceGrant relationships for permitted cross-namespace references
- Add reverse EndpointSlice to Service relationships
- Add HPA to scale target relationships
- Add NetworkPolicy to selected Pod relationships

## CloudNativePG

- Add Database row projection and Applied health semantics
- Add DatabaseRole row projection and Applied health semantics
- Add Publication and Subscription row projections
- Add FailoverQuorum row projection
- Add ImageCatalog and ClusterImageCatalog row projections
- Add Barman ObjectStore row projection and health semantics
- Add Cluster detail summary with topology, primary, image, storage, and replication health
- Add Instances and Backups tabs to Cluster details
- Add Backup and ScheduledBackup detail summaries
- Add safe create-backup and schedule suspend/resume actions with semantic RBAC checks
- Detect under-provisioned Poolers instead of relying only on phase

## Rook Ceph

- Add CephFilesystemSubVolumeGroup row projection
- Add projections for active Rook resources such as CephClient and ObjectBucket when deployed
- Add CephCluster detail summary with health, version, quorum, daemon counts, capacity, and health messages
- Add pool failure-domain, replication, and device-class details
- Add filesystem and object-store endpoint details

## Kopiur And Tuppr

- Give Kopiur and Tuppr separate categories and visual identities
- Add SnapshotPolicy, SnapshotSchedule, Snapshot, Repository, Restore, and replication row projections
- Add backup freshness, verification, suspension, and failure health semantics
- Add Kopiur policy and snapshot detail summaries
- Add restore history and progress summaries
- Add KubernetesUpgrade and TalosUpgrade row projections and detail summaries
- Add Kopiur Snapshot Now to bulk actions where valid

## Monitoring And Alerts

- Add a first-class Prometheus Operator category
- Add Prometheus, PrometheusAgent, Alertmanager, and ThanosRuler readiness projections
- Add PrometheusRule group, rule count, evaluation, and error summaries
- Add ServiceMonitor, PodMonitor, Probe, and ScrapeConfig target summaries
- Resolve Alertmanager labels to canonical Kubernetes resource targets
- Support standard workload, Pod, Service, Node, and PVC alert labels
- Support kube-state-metrics custom-resource labels
- Support Flux, CloudNativePG, Rook, cert-manager, and External Secrets alert labels
- Link firing alerts to their defining PrometheusRule
- Present a target chooser when alert labels resolve ambiguously

## Networking

- Add Cilium LoadBalancer IP pool availability and conflict projection
- Add Cilium BGP cluster, peer, and node readiness projections
- Add Cilium policy and endpoint health summaries
- Add Gateway API ReferenceGrant and BackendTLSPolicy projections
- Add Envoy Gateway traffic, security, extension, and backend policy summaries

## Core Resources

- Add VolumeAttachment projection with attach and detach errors
- Add ResourceQuota used/hard utilization columns and thresholds
- Add CertificateSigningRequest approval, denial, and failure status
- Add safe CertificateSigningRequest approve and deny actions
- Add CronJob last schedule, last success, active Job, and missed-schedule status
- Add CronJob suspend and resume actions
- Add CustomResourceDefinition establishment, naming, termination, and served-version status
- Add ReplicationController projection, scaling, logs, and relationships
- Add richer NetworkPolicy selector and rule details

## Details And Mobile

- Add operator-specific detail summaries for Flux, External Secrets, Kopiur, Tuppr, Rook, and CloudNativePG
- Make referenced resources in detail summaries clickable
- Reuse category icons and colors in mobile resource trees
- Define per-GVK mobile summary columns
- Generate desktop and mobile actions from the same capability registry
- Add missing mobile bulk actions for External Secrets, Kopiur, and CronJobs

## Testing

- Add table-driven discovery-family and projector-coverage tests
- Add unit tests for generic status, core, Flux, External Secrets, Gateway API, and RBAC projectors
- Add layout integration tests for Rook, CloudNativePG, Kopiur, Tuppr, and monitoring resources
- Install representative operator CRDs in integration tests
- Exercise operator discovery and projected columns in browser tests
