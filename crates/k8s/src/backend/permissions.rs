//! RBAC permission checks via short-TTL cached SelfSubjectAccessReviews.

use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use kube::api::{Api, PostParams};
use roder_core::{
    AccessRow, ActionPermissions, DrainOptions, ResourceAction, ResourceKind, SanitizeAction,
    SweepOptions, ACCESS_REVIEW_OPERATIONS,
};

use super::Backend;

impl Backend {
    /// RBAC: whether the current identity may use a verb on this kind and namespace.
    pub async fn can(&self, verb: &str, key: &str, ns: Option<&str>) -> bool {
        self.can_resource(verb, key, ns, None, None).await
    }

    pub async fn can_named(&self, verb: &str, key: &str, ns: Option<&str>, name: &str) -> bool {
        self.can_resource(verb, key, ns, Some(name), None).await
    }

    pub async fn can_subresource(
        &self,
        verb: &str,
        key: &str,
        ns: Option<&str>,
        subresource: &str,
    ) -> bool {
        self.can_resource(verb, key, ns, None, Some(subresource))
            .await
    }

    pub async fn can_named_subresource(
        &self,
        verb: &str,
        key: &str,
        ns: Option<&str>,
        name: &str,
        subresource: &str,
    ) -> bool {
        self.can_resource(verb, key, ns, Some(name), Some(subresource))
            .await
    }

    async fn can_resource(
        &self,
        verb: &str,
        key: &str,
        ns: Option<&str>,
        name: Option<&str>,
        subresource: Option<&str>,
    ) -> bool {
        const TTL: std::time::Duration = std::time::Duration::from_secs(30);
        let Ok(entry) = self.entry(key) else {
            return false;
        };
        let ns = if entry.kind.namespaced { ns } else { None };
        let ck = (
            verb.to_string(),
            key.to_string(),
            ns.map(|s| s.to_string()),
            name.map(String::from),
            subresource.map(|value| value.to_string()),
        );
        {
            let cache = self.can_cache.read().await;
            if let Some((at, allowed)) = cache.get(&ck) {
                if at.elapsed() < TTL {
                    return *allowed;
                }
            }
        }
        let ssar = SelfSubjectAccessReview {
            spec: SelfSubjectAccessReviewSpec {
                resource_attributes: Some(ResourceAttributes {
                    verb: Some(verb.to_string()),
                    group: Some(entry.kind.group.clone()),
                    resource: Some(entry.kind.plural.clone()),
                    name: name.map(String::from),
                    subresource: subresource.map(|value| value.to_string()),
                    namespace: ns.map(|s| s.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let api: Api<SelfSubjectAccessReview> = Api::all(self.client());
        let allowed = match api.create(&PostParams::default(), &ssar).await {
            Ok(r) => r.status.map(|s| s.allowed).unwrap_or(false),
            Err(e) => {
                // Don't cache transient failures — a network blip would hide
                // all action buttons for 30 seconds on every affected resource.
                tracing::warn!("SSAR failed for {verb} on {key}: {e}");
                return false;
            }
        };
        let mut cache = self.can_cache.write().await;
        // Evict stale entries so the map doesn't grow O(verbs × kinds × namespaces).
        cache.retain(|_, (at, _)| at.elapsed() < TTL * 10);
        cache.insert(ck, (std::time::Instant::now(), allowed));
        allowed
    }

    pub async fn can_action(
        &self,
        action: ResourceAction,
        key: &str,
        ns: Option<&str>,
        name: Option<&str>,
        drain_options: Option<&DrainOptions>,
    ) -> bool {
        let Ok(kind) = self.resource_kind(key) else {
            return false;
        };
        if !kind.supports(action) {
            return false;
        }
        let target = |verb| async move {
            match name {
                Some(name) => self.can_named(verb, key, ns, name).await,
                None => self.can(verb, key, ns).await,
            }
        };
        let target_subresource = |verb, subresource| async move {
            match name {
                Some(name) => {
                    self.can_named_subresource(verb, key, ns, name, subresource)
                        .await
                }
                None => self.can_subresource(verb, key, ns, subresource).await,
            }
        };

        match action {
            ResourceAction::Delete => target("delete").await,
            ResourceAction::Evict => target_subresource("create", "eviction").await,
            ResourceAction::Scale
            | ResourceAction::Restart
            | ResourceAction::Cordon
            | ResourceAction::FluxReconcile
            | ResourceAction::FluxSuspend
            | ResourceAction::FluxForce
            | ResourceAction::FluxReset
            | ResourceAction::ExternalSecretsRefresh => target("patch").await,
            ResourceAction::Drain => {
                let pods = ResourceKind::make_key("", "v1", "Pod");
                let pod_action = if drain_options.is_some_and(|options| options.disable_eviction) {
                    self.can("delete", &pods, None).await
                } else {
                    self.can_subresource("create", &pods, None, "eviction")
                        .await
                };
                (match name {
                    Some(name) => self.can_named("patch", key, None, name).await,
                    None => self.can("patch", key, None).await,
                }) && self.can("list", &pods, None).await
                    && pod_action
            }
            ResourceAction::Logs => self.can_logs(&kind, key, ns, name).await,
            ResourceAction::Exec => target_subresource("create", "exec").await,
            ResourceAction::DebugExec => {
                target("get").await
                    && target_subresource("patch", "ephemeralcontainers").await
                    && target_subresource("create", "exec").await
            }
            ResourceAction::NodeShell => {
                let pods = ResourceKind::make_key("", "v1", "Pod");
                let namespace = Some(super::exec::NODE_SHELL_NAMESPACE);
                target("get").await
                    && self.can("create", &pods, namespace).await
                    && self.can("get", &pods, namespace).await
                    && self.can("delete", &pods, namespace).await
                    && self
                        .can_subresource("create", &pods, namespace, "exec")
                        .await
            }
            ResourceAction::FluxReconcileWithSource => {
                if !target("get").await || !target("patch").await {
                    return false;
                }
                let Some(name) = name else {
                    return false;
                };
                let Ok(source) = self.flux_source_target(key, ns, name).await else {
                    return false;
                };
                self.can_named(
                    "patch",
                    &source.key,
                    source.namespace.as_deref(),
                    &source.name,
                )
                .await
            }
            ResourceAction::CertificateRenew => {
                target("get").await && target_subresource("update", "status").await
            }
            ResourceAction::CronJobTrigger | ResourceAction::JobRerun => {
                let jobs = ResourceKind::make_key("batch", "v1", "Job");
                target("get").await && self.can("create", &jobs, ns).await
            }
            ResourceAction::KopiurSnapshotNow => {
                let snapshot = ResourceKind::make_key(&kind.group, &kind.version, "Snapshot");
                self.can("create", &snapshot, ns).await
            }
        }
    }

    pub async fn can_sanitize(
        &self,
        namespace: Option<&str>,
        options: SweepOptions,
        action: SanitizeAction,
    ) -> bool {
        let verb_allowed = |key: String| async move {
            self.can("list", &key, namespace).await
                && (action == SanitizeAction::Preview || self.can("delete", &key, namespace).await)
        };
        if options.includes_pods() && !verb_allowed(ResourceKind::make_key("", "v1", "Pod")).await {
            return false;
        }
        if options.includes_jobs()
            && !verb_allowed(ResourceKind::make_key("batch", "v1", "Job")).await
        {
            return false;
        }
        true
    }

    async fn can_logs(
        &self,
        kind: &ResourceKind,
        key: &str,
        ns: Option<&str>,
        name: Option<&str>,
    ) -> bool {
        let pods = ResourceKind::make_key("", "v1", "Pod");
        if kind.group.is_empty() && kind.kind == "Pod" {
            match name {
                Some(name) => {
                    self.can_named("get", key, ns, name).await
                        && self
                            .can_named_subresource("get", key, ns, name, "log")
                            .await
                }
                None => {
                    self.can("get", key, ns).await
                        && self.can_subresource("get", key, ns, "log").await
                }
            }
        } else {
            let target = match name {
                Some(name) => self.can_named("get", key, ns, name).await,
                None => self.can("get", key, ns).await,
            };
            target
                && self.can("list", &pods, ns).await
                && self.can_subresource("get", &pods, ns, "log").await
        }
    }

    pub async fn action_permissions(
        &self,
        key: &str,
        ns: Option<&str>,
        name: Option<&str>,
    ) -> ActionPermissions {
        let Ok(kind) = self.resource_kind(key) else {
            return ActionPermissions::default();
        };
        let mut actions = std::collections::BTreeMap::new();
        for action in ResourceAction::ALL {
            if kind.supports(action) {
                actions.insert(
                    action.api_name().to_string(),
                    self.can_action(action, key, ns, name, None).await,
                );
            }
        }
        ActionPermissions {
            apply: self.can("patch", key, ns).await,
            actions,
        }
    }

    /// "What can I do?" across every known resource kind, given OIDC
    /// passthrough — the per-kind `can()` calls run concurrently and share
    /// its cache, so re-opening the review shortly after is cheap.
    pub async fn access_review(&self, ns: Option<&str>) -> Vec<AccessRow> {
        let futs = self.kinds().into_iter().map(|k| async move {
            let capabilities = k.capabilities();
            let mut operations = Vec::with_capacity(ACCESS_REVIEW_OPERATIONS.len());
            for verb in ["get", "list", "watch", "create", "patch", "delete"] {
                operations.push((verb.to_string(), Some(self.can(verb, &k.key, ns).await)));
            }
            operations.push((
                "status".to_string(),
                Some(self.can_subresource("update", &k.key, ns, "status").await),
            ));
            for (label, action) in [
                ("logs", ResourceAction::Logs),
                ("exec", ResourceAction::Exec),
                ("evict", ResourceAction::Evict),
            ] {
                operations.push((
                    label.to_string(),
                    if capabilities.supports(action) {
                        Some(self.can_action(action, &k.key, ns, None, None).await)
                    } else {
                        None
                    },
                ));
            }
            let dependent_actions = [
                ResourceAction::Drain,
                ResourceAction::CronJobTrigger,
                ResourceAction::JobRerun,
                ResourceAction::KopiurSnapshotNow,
            ];
            let mut applicable = false;
            let mut dependencies_allowed = true;
            for action in dependent_actions {
                if capabilities.supports(action) {
                    applicable = true;
                    dependencies_allowed &= self.can_action(action, &k.key, ns, None, None).await;
                }
            }
            operations.push((
                "deps".to_string(),
                applicable.then_some(dependencies_allowed),
            ));
            AccessRow {
                kind: k.kind,
                group: k.group,
                namespaced: k.namespaced,
                category: k.category,
                operations,
            }
        });
        let mut rows = futures::future::join_all(futs).await;
        rows.sort_by(|a, b| {
            a.category
                .order()
                .cmp(&b.category.order())
                .then_with(|| a.category.label().cmp(&b.category.label()))
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.group.cmp(&b.group))
        });
        rows
    }
}
