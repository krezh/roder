# TODO

## Actions And Access

- Group Access Review rows by category or operator

## Resource Relationships

- Add ObjectBucketClaim to ObjectBucket, Secret, and ConfigMap relationships
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
- Add HPA to scale target relationships
- Add NetworkPolicy to selected Pod relationships

## CloudNativePG

- Add Database row projection and Applied health semantics
- Add DatabaseRole row projection and Applied health semantics
- Add Publication and Subscription row projections
- Add FailoverQuorum row projection
- Add ImageCatalog and ClusterImageCatalog row projections
- Add Barman ObjectStore row projection and health semantics
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

## Monitoring And Alerts

- Add PrometheusRule group, rule count, evaluation, and error summaries
- Add ServiceMonitor, PodMonitor, Probe, and ScrapeConfig target summaries
- Support Flux, CloudNativePG, Rook, cert-manager, and External Secrets alert labels
- Present a target chooser when alert labels resolve ambiguously

## Networking

- Add Cilium LoadBalancer IP pool availability and conflict projection
- Add Cilium BGP cluster, peer, and node readiness projections
- Add Cilium policy and endpoint health summaries
- Add Gateway API ReferenceGrant and BackendTLSPolicy projections
- Add Envoy Gateway traffic, security, extension, and backend policy summaries

## Core Resources

- Add ResourceQuota used/hard utilization columns and thresholds
- Add CertificateSigningRequest approval, denial, and failure status
- Add safe CertificateSigningRequest approve and deny actions
- Add CronJob suspend and resume actions
- Add CustomResourceDefinition establishment, naming, termination, and served-version status
- Add ReplicationController projection, scaling, logs, and relationships
- Add richer NetworkPolicy selector and rule details

## Details And Mobile

- Add operator-specific detail summaries for Flux, External Secrets, Kopiur, Tuppr, and Rook
- Make referenced resources in detail summaries clickable
- Reuse category icons and colors in mobile resource trees
- Define per-GVK mobile summary columns

## Testing

- Add layout integration tests for Kopiur and Tuppr resources
