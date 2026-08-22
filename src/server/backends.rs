//! Per-subject backend registry: resolves an authenticated caller's `Tokens`
//! to their own `Arc<Backend>`, building it on first use (single-flighted per
//! subject so concurrent requests from the same user don't each run a full
//! connect + discovery). This is the live request-path source of each
//! caller's `Backend`: `require_auth` resolves it here and passes it to
//! handlers via request extensions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use roder_auth::{OidcProvider, Tokens};
use roder_k8s::{Backend, SharedCluster};
use tokio::sync::{Mutex, OnceCell, RwLock};

struct Entry {
    backend: Arc<Backend>,
    tokens: Tokens,
    last_active: Instant,
}

#[derive(Clone)]
pub struct ResolvedBackend {
    pub backend: Arc<Backend>,
    pub tokens: Tokens,
}

/// Backend resolution failed (missing/expired credentials, no OIDC provider
/// configured, or the cluster connect/token-refresh itself failed). Callers
/// only branch on success vs failure today, but this is a real error type —
/// not `()` — so it composes with `?`/`std::error::Error` if a caller ever
/// needs to report the cause.
#[derive(Debug)]
pub struct ResolveError;

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("backend resolution failed")
    }
}

impl std::error::Error for ResolveError {}

/// Maps an authenticated subject to their own `Arc<Backend>`, building it
/// lazily on first use. Wired into `AppState.backends` and resolved by
/// `require_auth` on every authenticated request.
pub struct BackendRegistry {
    shared: Arc<SharedCluster>,
    /// Used by the production `mint_tokens` refresh path; unread under
    /// `#[cfg(test)]`, where `mint_tokens` is a no-op.
    #[cfg_attr(test, allow(dead_code))]
    provider: Option<Arc<OidcProvider>>,
    /// Max number of concurrently-cached per-subject backends. Soft: enforced
    /// by evicting idle entries in `enforce_cap`, but a new entrant is never
    /// rejected — if every cached entry is actively subscribed, the registry
    /// is allowed to run over cap (see `enforce_cap`).
    cap: usize,
    /// Idle eviction threshold: a subject with no active SSE subscribers
    /// whose `last_active` is older than this is dropped by the reaper.
    idle: Duration,
    map: RwLock<HashMap<String, Entry>>,
    build_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Dev-mode's single implicit backend, built once (via default/inferred
    /// kubeconfig creds — there's no OIDC token in dev) and reused for every
    /// request. Kept separate from `map` since it isn't keyed by a real
    /// subject and never expires/refreshes.
    dev_backend: OnceCell<Arc<Backend>>,
}

impl BackendRegistry {
    pub fn new(
        shared: Arc<SharedCluster>,
        provider: Option<Arc<OidcProvider>>,
        cap: usize,
        idle: Duration,
    ) -> Self {
        Self {
            shared,
            provider,
            cap,
            idle,
            map: RwLock::new(HashMap::new()),
            build_locks: Mutex::new(HashMap::new()),
            dev_backend: OnceCell::new(),
        }
    }

    /// Resolve dev mode's single implicit backend, building it once (via
    /// default/inferred kubeconfig creds — dev bypasses OIDC entirely, so
    /// there's no bearer token to pass through) and reusing it thereafter.
    pub async fn resolve_dev(&self) -> Result<Arc<Backend>, ResolveError> {
        self.dev_backend
            .get_or_try_init(|| self.build_dev_backend())
            .await
            .cloned()
    }

    #[cfg(not(test))]
    async fn build_dev_backend(&self) -> Result<Arc<Backend>, ResolveError> {
        Backend::connect_with_default(self.shared.clone())
            .await
            .map(Arc::new)
            .map_err(|_| ResolveError)
    }

    #[cfg(test)]
    async fn build_dev_backend(&self) -> Result<Arc<Backend>, ResolveError> {
        Ok(Arc::new(Backend::from_parts_for_test(
            roder_k8s::ClusterAccess::for_test(),
            self.shared.clone(),
        )))
    }

    // `is_empty` isn't needed by any caller yet; `len` exists mainly for
    // test observability (registry size after resolves/evictions).
    #[allow(clippy::len_without_is_empty)]
    pub async fn len(&self) -> usize {
        self.map.read().await.len()
    }

    /// Drop a subject's cached backend (logout). See `evict_locked` for why
    /// this can be a (rare, self-healing) no-op.
    pub async fn evict(&self, subject: &str) {
        self.evict_locked(subject).await;
    }

    /// Read back a subject's current cached token set.
    pub async fn resolved_tokens(&self, subject: &str) -> Option<Tokens> {
        self.map.read().await.get(subject).map(|e| e.tokens.clone())
    }

    /// Drop all per-user backends (e.g. on shutdown), shedding their informers/watches.
    pub async fn clear(&self) {
        self.map.write().await.clear();
    }

    async fn subject_lock(&self, subject: &str) -> Arc<Mutex<()>> {
        let mut locks = self.build_locks.lock().await;
        locks
            .entry(subject.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn cleanup_subject_lock(&self, subject: &str, lock: &Arc<Mutex<()>>) {
        let mut locks = self.build_locks.lock().await;
        if locks
            .get(subject)
            .is_some_and(|stored| Arc::ptr_eq(stored, lock))
            && Arc::strong_count(lock) == 2
        {
            locks.remove(subject);
        }
    }

    /// Evict a subject's cached entry, but only if no build/refresh is
    /// currently in flight for it. `resolve`'s cold path holds this
    /// subject's build lock (from `subject_lock`) for its entire
    /// mint-token + connect + `insert` sequence; if we removed the map entry
    /// while that was happening, the in-flight build's own `insert()` would
    /// re-add the entry right after, silently undoing the eviction.
    ///
    /// We close that race by taking the *same* lock here: `try_lock`
    /// uncontended (no build in flight for this subject) evicts immediately;
    /// if it's held, a build is genuinely in progress, so we skip this pass
    /// rather than block the reaper/cap-enforcement/caller on someone else's
    /// connect — idle eviction retries on the next 15s tick, and a cap
    /// breach simply tries the next-LRU idle candidate instead. We do not
    /// wait for the lock: the informer-level idle timers plus never-reject
    /// semantics mean a missed eviction this pass has no correctness impact,
    /// only a slightly later cleanup.
    ///
    async fn evict_locked(&self, subject: &str) -> bool {
        let lock = self.build_locks.lock().await.get(subject).cloned();
        let guard = match &lock {
            Some(l) => match l.try_lock() {
                Ok(guard) => Some(guard),
                Err(_) => return false, // build/refresh in flight; skip this pass
            },
            None => None, // subject was never resolved; nothing to race
        };
        let removed = self.map.write().await.remove(subject).is_some();
        drop(guard);
        if let Some(lock) = &lock {
            self.cleanup_subject_lock(subject, lock).await;
        }
        removed
    }

    /// Resolve the caller's own `Backend`, building it (single-flight per
    /// subject) and refreshing the token if it's near expiry. Records
    /// last-activity on every successful resolve.
    pub async fn resolve(&self, tokens: &Tokens) -> Result<ResolvedBackend, ResolveError> {
        let subject = tokens.identity.subject.clone();
        if subject.is_empty() {
            return Err(ResolveError);
        }

        // Fast path: warm, valid backend for this subject.
        if let Some(b) = self.warm(&subject).await {
            return Ok(b);
        }

        // Single-flight build/refresh per subject.
        let lock = self.subject_lock(&subject).await;
        let guard = lock.lock().await;
        let result = if let Some(resolved) = self.warm(&subject).await {
            Ok(resolved)
        } else {
            self.refresh_or_build(subject.clone(), tokens).await
        };
        drop(guard);
        self.cleanup_subject_lock(&subject, &lock).await;
        result
    }

    /// Resolve token values produced directly by a successful, verified OIDC
    /// exchange. Unlike browser-cookie resolution, this may replace a warm
    /// identity because the caller has just verified the ID token.
    pub async fn resolve_login(&self, tokens: &Tokens) -> Result<ResolvedBackend, ResolveError> {
        let subject = tokens.identity.subject.clone();
        if subject.is_empty() || tokens.id_token.is_empty() {
            return Err(ResolveError);
        }

        let lock = self.subject_lock(&subject).await;
        let guard = lock.lock().await;
        let result = self.replace_login(subject.clone(), tokens).await;
        drop(guard);
        self.cleanup_subject_lock(&subject, &lock).await;
        result
    }

    async fn replace_login(
        &self,
        subject: String,
        tokens: &Tokens,
    ) -> Result<ResolvedBackend, ResolveError> {
        let mut map = self.map.write().await;
        if let Some(entry) = map.get_mut(&subject) {
            entry
                .backend
                .set_token(&tokens.id_token)
                .map_err(|_| ResolveError)?;
            entry.tokens = tokens.clone();
            entry.last_active = Instant::now();
            return Ok(ResolvedBackend {
                backend: entry.backend.clone(),
                tokens: entry.tokens.clone(),
            });
        }
        drop(map);
        let backend = self.build_backend(tokens).await?;
        self.insert(subject, backend.clone(), tokens.clone()).await;
        Ok(ResolvedBackend {
            backend,
            tokens: tokens.clone(),
        })
    }

    async fn refresh_or_build(
        &self,
        subject: String,
        tokens: &Tokens,
    ) -> Result<ResolvedBackend, ResolveError> {
        let live = self.mint_tokens(tokens).await?;
        {
            let mut map = self.map.write().await;
            if let Some(entry) = map.get_mut(&subject) {
                entry
                    .backend
                    .set_token(&live.id_token)
                    .map_err(|_| ResolveError)?;
                entry.tokens = live;
                entry.last_active = Instant::now();
                return Ok(ResolvedBackend {
                    backend: entry.backend.clone(),
                    tokens: entry.tokens.clone(),
                });
            }
        }

        let backend = self.build_backend(&live).await?;
        self.insert(subject, backend.clone(), live.clone()).await;
        Ok(ResolvedBackend {
            backend,
            tokens: live,
        })
    }

    async fn warm(&self, subject: &str) -> Option<ResolvedBackend> {
        let mut map = self.map.write().await;
        let e = map.get_mut(subject)?;
        // Only reuse for the same subject (guaranteed by key) and if not near expiry.
        if e.tokens.needs_refresh() || e.tokens.id_token.is_empty() {
            return None;
        }
        e.last_active = Instant::now();
        Some(ResolvedBackend {
            backend: e.backend.clone(),
            tokens: e.tokens.clone(),
        })
    }

    async fn insert(&self, subject: String, backend: Arc<Backend>, tokens: Tokens) {
        self.map.write().await.insert(
            subject,
            Entry {
                backend,
                tokens,
                last_active: Instant::now(),
            },
        );
        self.enforce_cap().await;
    }

    /// Soft LRU cap: while over `self.cap`, evict the least-recently-active
    /// entry that currently has no live SSE subscribers, oldest first. If no
    /// entry is idle (every cached backend is actively being watched), stop
    /// and log a warning instead of evicting an active user or rejecting the
    /// new one — the registry is allowed to run over cap rather than either.
    async fn enforce_cap(&self) {
        loop {
            let len = self.map.read().await.len();
            if len <= self.cap {
                return;
            }

            // Snapshot (subject, last_active, backend) outside the map lock
            // so the `has_active_subscribers` awaits below don't hold it.
            let mut candidates: Vec<(String, Instant, Arc<Backend>)> = self
                .map
                .read()
                .await
                .iter()
                .map(|(k, e)| (k.clone(), e.last_active, e.backend.clone()))
                .collect();
            candidates.sort_by_key(|(_, last_active, _)| *last_active);

            let mut evicted = false;
            for (subject, _, backend) in candidates {
                if backend.has_active_subscribers().await {
                    continue;
                }
                if self.evict_locked(&subject).await {
                    evicted = true;
                    break; // re-snapshot: map/len changed
                }
            }

            if !evicted {
                tracing::warn!(
                    cap = self.cap,
                    len,
                    "BackendRegistry over soft cap with no idle entries to evict; admitting anyway"
                );
                return;
            }
        }
    }

    /// One idle-reaper pass: evict subjects whose `last_active` is past
    /// `self.idle` AND that currently have no live SSE subscribers. Shared by
    /// `spawn_reaper`'s background loop and the `#[cfg(test)]` `reap_once`
    /// seam so tests can drive a single deterministic pass.
    async fn reap_pass(&self) {
        let candidates: Vec<(String, Arc<Backend>)> = self
            .map
            .read()
            .await
            .iter()
            .filter(|(_, e)| e.last_active.elapsed() >= self.idle)
            .map(|(k, e)| (k.clone(), e.backend.clone()))
            .collect();

        for (subject, backend) in candidates {
            if backend.has_active_subscribers().await {
                continue; // still actively watched; not truly idle
            }
            self.evict_locked(&subject).await;
        }
    }

    /// Start the background idle-eviction reaper: one `reap_pass` every 15s.
    /// Not folded into `new` because `new` returns a bare `Self` (existing
    /// callers construct then wrap in `Arc` themselves); call this once after
    /// wrapping the registry in an `Arc`.
    pub fn spawn_reaper(self: &Arc<Self>) {
        let registry = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            loop {
                tick.tick().await;
                registry.reap_pass().await;
            }
        });
    }

    /// Refresh via the IdP when the token is empty/near expiry (ported from
    /// `handlers::middleware::ensure_session`'s refresh branch). In tests this
    /// is a no-op: tokens are returned unchanged.
    #[cfg(not(test))]
    async fn mint_tokens(&self, tokens: &Tokens) -> Result<Tokens, ResolveError> {
        if tokens.id_token.is_empty() || tokens.needs_refresh() {
            let provider = self.provider.clone().ok_or(ResolveError)?;
            let rt = tokens.refresh_token.clone().ok_or(ResolveError)?;
            provider.refresh(rt).await.map_err(|_| ResolveError)
        } else {
            Ok(tokens.clone())
        }
    }

    #[cfg(test)]
    async fn mint_tokens(&self, tokens: &Tokens) -> Result<Tokens, ResolveError> {
        Ok(tokens.clone())
    }

    #[cfg(not(test))]
    async fn build_backend(&self, tokens: &Tokens) -> Result<Arc<Backend>, ResolveError> {
        Backend::connect_with_token(&tokens.id_token, self.shared.clone())
            .await
            .map(Arc::new)
            .map_err(|_| ResolveError)
    }

    #[cfg(test)]
    async fn build_backend(&self, _tokens: &Tokens) -> Result<Arc<Backend>, ResolveError> {
        Ok(Arc::new(Backend::from_parts_for_test(
            roder_k8s::ClusterAccess::for_test(),
            self.shared.clone(),
        )))
    }

    /// Test-only: force a subject's cached tokens into the past so the next
    /// `resolve` takes the refresh-in-place path instead of the warm fast path.
    #[cfg(test)]
    pub async fn force_expire(&self, subject: &str) {
        if let Some(e) = self.map.write().await.get_mut(subject) {
            e.tokens.expires_at = time::OffsetDateTime::now_utc() - time::Duration::hours(2);
        }
    }

    /// Test-only: drive a single reaper pass synchronously instead of
    /// waiting on `spawn_reaper`'s 15s background tick.
    #[cfg(test)]
    pub async fn reap_once(&self) {
        self.reap_pass().await;
    }

    /// Test-only: look up a subject's cached backend without going through
    /// `resolve` (which would refresh `last_active` and mask eviction).
    #[cfg(test)]
    pub async fn get_backend(&self, subject: &str) -> Option<Arc<Backend>> {
        self.map
            .read()
            .await
            .get(subject)
            .map(|e| e.backend.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use roder_auth::{Identity, Tokens};
    use roder_k8s::SharedCluster;
    use time::OffsetDateTime;

    use super::BackendRegistry;

    fn fake_tokens() -> Tokens {
        Tokens {
            id_token: "id".into(),
            access_token: "access".into(),
            refresh_token: Some("rt".into()),
            expires_at: OffsetDateTime::now_utc() + time::Duration::seconds(3600),
            identity: Identity {
                subject: "sub".into(),
                email: Some("a@b".into()),
                name: Some("Alice".into()),
                groups: vec!["admins".into()],
            },
        }
    }

    #[tokio::test]
    async fn resolve_is_stable_per_subject() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));
        let mut a1 = fake_tokens();
        a1.identity.subject = "alice".into();
        let mut b1 = fake_tokens();
        b1.identity.subject = "bob".into();

        let a = reg.resolve(&a1).await.unwrap();
        let a_again = reg.resolve(&a1).await.unwrap();
        let b = reg.resolve(&b1).await.unwrap();

        assert!(
            Arc::ptr_eq(&a.backend, &a_again.backend),
            "same subject -> same backend"
        );
        assert!(
            !Arc::ptr_eq(&a.backend, &b.backend),
            "distinct subjects -> distinct backends"
        );
        assert_eq!(reg.len().await, 2);
    }

    #[tokio::test]
    async fn near_expiry_entry_is_refreshed_and_token_swapped() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));
        let mut t = fake_tokens();
        t.identity.subject = "alice".into();

        let b = reg.resolve(&t).await.unwrap();
        reg.force_expire("alice").await;
        let b2 = reg.resolve(&t).await.unwrap();

        assert!(
            Arc::ptr_eq(&b.backend, &b2.backend),
            "refresh reuses the same backend, only swaps the token"
        );

        // `force_expire` set expires_at to 2h in the past; if `resolve`'s
        // refresh path failed to write `entry.tokens = live` back into the
        // map, the stored expiry would still be in the past here and
        // `needs_refresh()` would never clear (refresh-storm bug).
        let expires_at = reg.map.read().await.get("alice").unwrap().tokens.expires_at;
        assert!(
            expires_at > OffsetDateTime::now_utc(),
            "entry.tokens must be updated to the fresh set after refresh, not left stale"
        );
    }

    #[tokio::test]
    async fn evict_drops_only_the_named_subject() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));
        let mut a1 = fake_tokens();
        a1.identity.subject = "alice".into();
        let mut b1 = fake_tokens();
        b1.identity.subject = "bob".into();

        let a_before = reg.resolve(&a1).await.unwrap();
        reg.resolve(&b1).await.unwrap();
        assert_eq!(reg.len().await, 2);

        reg.evict("bob").await;
        assert_eq!(reg.len().await, 1);

        // The surviving subject still resolves to the same cached backend.
        let a_after = reg.resolve(&a1).await.unwrap();
        assert!(Arc::ptr_eq(&a_before.backend, &a_after.backend));
    }

    #[tokio::test]
    async fn completed_resolve_cleans_unused_build_lock() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));
        let mut a1 = fake_tokens();
        a1.identity.subject = "alice".into();

        reg.resolve(&a1).await.unwrap();
        assert!(
            !reg.build_locks.lock().await.contains_key("alice"),
            "unused lock entries should not accumulate"
        );
    }

    #[tokio::test]
    async fn evict_skips_a_subject_with_an_in_flight_build() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));
        let mut a1 = fake_tokens();
        a1.identity.subject = "alice".into();
        reg.resolve(&a1).await.unwrap();

        // Simulate a concurrent resolve() mid-build by holding the subject's
        // build lock, exactly as `resolve`'s cold path does for its whole
        // mint+connect+insert sequence.
        let lock = reg.subject_lock("alice").await;
        let guard = lock.lock().await;
        let same_lock = reg.subject_lock("alice").await;
        assert!(Arc::ptr_eq(&lock, &same_lock));

        reg.evict("alice").await;
        assert!(
            reg.get_backend("alice").await.is_some(),
            "evict must skip a subject whose build lock is currently held, not race it"
        );

        drop(guard);
        drop(same_lock);
        drop(lock);
        reg.evict("alice").await; // uncontended now — proceeds normally
        assert!(reg.get_backend("alice").await.is_none());
        assert!(!reg.build_locks.lock().await.contains_key("alice"));
    }

    #[tokio::test]
    async fn reaper_evicts_idle_and_soft_cap_never_rejects() {
        let shared = SharedCluster::for_test();
        let reg = Arc::new(BackendRegistry::new(
            shared,
            None,
            1,
            Duration::from_millis(50),
        ));
        let mut a = fake_tokens();
        a.identity.subject = "alice".into();
        let mut b = fake_tokens();
        b.identity.subject = "bob".into();

        reg.resolve(&a).await.unwrap();
        reg.reap_once().await; // #[cfg(test)] single reaper pass
        assert_eq!(reg.len().await, 1); // still active-ish; not past idle yet
        tokio::time::sleep(Duration::from_millis(60)).await;
        // cap=1: resolving bob evicts idle alice.
        reg.resolve(&b).await.unwrap();
        assert!(reg.len().await <= 1);
        assert!(
            reg.get_backend("bob").await.is_some(),
            "soft cap never rejects the new user"
        );
    }

    #[tokio::test]
    async fn resolve_dev_builds_once_and_is_reused() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));

        let a = reg.resolve_dev().await.unwrap();
        let b = reg.resolve_dev().await.unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "dev backend must be built once and reused"
        );
    }

    #[tokio::test]
    async fn resolved_tokens_reflects_the_stored_entry() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));
        let mut t = fake_tokens();
        t.identity.subject = "alice".into();

        assert!(reg.resolved_tokens("alice").await.is_none());
        reg.resolve(&t).await.unwrap();
        let stored = reg.resolved_tokens("alice").await.unwrap();
        assert_eq!(stored.identity.subject, "alice");
    }

    #[tokio::test]
    async fn stale_cookie_groups_do_not_replace_a_warm_identity() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));
        let mut current = fake_tokens();
        current.identity.subject = "alice".into();
        current.identity.groups = vec!["viewers".into()];

        let before = reg.resolve_login(&current).await.unwrap();

        let mut stale_cookie = current.clone();
        stale_cookie.id_token.clear();
        stale_cookie.access_token.clear();
        stale_cookie.identity.groups = vec!["admins".into()];
        stale_cookie.refresh_token = Some("stale-refresh-token".into());
        let after = reg.resolve(&stale_cookie).await.unwrap();

        assert!(
            Arc::ptr_eq(&before.backend, &after.backend),
            "warm resolve should reuse the backend"
        );
        assert_eq!(after.tokens.identity.groups, vec!["viewers"]);
        assert_eq!(after.tokens.refresh_token.as_deref(), Some("rt"));
    }

    #[tokio::test]
    async fn verified_login_replaces_a_warm_identity() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));
        let mut previous = fake_tokens();
        previous.identity.subject = "alice".into();
        previous.identity.groups = vec!["admins".into()];
        reg.resolve_login(&previous).await.unwrap();

        let mut login = previous.clone();
        login.id_token = "new-verified-id-token".into();
        login.identity.groups = vec!["viewers".into()];
        let resolved = reg.resolve_login(&login).await.unwrap();

        assert_eq!(resolved.tokens.identity.groups, vec!["viewers"]);
        assert_eq!(resolved.tokens.id_token, "new-verified-id-token");
    }
}
