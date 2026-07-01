//! RBAC permission checks via SelfSubjectAccessReview, short-TTL cached so the
//! per-detail-open `patch`+`delete` checks don't each round-trip the apiserver.

use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use kube::api::{Api, PostParams};

use super::Backend;

impl Backend {
    /// RBAC: which actions may the current identity take on this kind/namespace.
    /// Cached briefly so the per-detail-open `patch`+`delete` checks don't each SSAR.
    pub async fn can(&self, verb: &str, key: &str, ns: Option<&str>) -> bool {
        const TTL: std::time::Duration = std::time::Duration::from_secs(30);
        let ck = (verb.to_string(), key.to_string(), ns.map(|s| s.to_string()));
        {
            let cache = self.can_cache.read().await;
            if let Some((at, allowed)) = cache.get(&ck) {
                if at.elapsed() < TTL {
                    return *allowed;
                }
            }
        }
        let Ok(entry) = self.entry(key) else {
            return false;
        };
        let ssar = SelfSubjectAccessReview {
            spec: SelfSubjectAccessReviewSpec {
                resource_attributes: Some(ResourceAttributes {
                    verb: Some(verb.to_string()),
                    group: Some(entry.kind.group.clone()),
                    resource: Some(entry.kind.plural.clone()),
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
}
