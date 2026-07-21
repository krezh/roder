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
    pub async fn resolve_dev(&self) -> Result<Arc<Backend>, ()> {
        self.dev_backend
            .get_or_try_init(|| self.build_dev_backend())
            .await
            .cloned()
    }

    #[cfg(not(test))]
    async fn build_dev_backend(&self) -> Result<Arc<Backend>, ()> {
        Backend::connect_with_default(self.shared.clone())
            .await
            .map(Arc::new)
            .map_err(|_| ())
    }

    #[cfg(test)]
    async fn build_dev_backend(&self) -> Result<Arc<Backend>, ()> {
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

    /// Read back a subject's current cached token set (post-`resolve`),
    /// e.g. so `require_auth` can re-seal the possibly-refreshed/rotated
    /// tokens into the session cookie without `resolve` itself needing to
    /// return anything beyond the `Arc<Backend>` (Tasks 5/6 tests depend on
    /// that signature).
    pub async fn resolved_tokens(&self, subject: &str) -> Option<Tokens> {
        self.map.read().await.get(subject).map(|e| e.tokens.clone())
    }

    /// Drop all per-user backends (e.g. on shutdown), shedding their informers/watches.
    pub async fn clear(&self) {
        self.map.write().await.clear();
        self.build_locks.lock().await.clear();
    }

    async fn subject_lock(&self, subject: &str) -> Arc<Mutex<()>> {
        let mut locks = self.build_locks.lock().await;
        locks
            .entry(subject.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
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
    /// Once the lock is acquired we also drop the subject's `build_locks`
    /// entry (not just the `map` entry) so that map churns don't grow
    /// `build_locks` unboundedly; a fresh lock is lazily recreated by
    /// `subject_lock` next time this subject resolves.
    async fn evict_locked(&self, subject: &str) -> bool {
        let lock = self.build_locks.lock().await.get(subject).cloned();
        let _guard = match &lock {
            Some(l) => match l.try_lock() {
                Ok(guard) => Some(guard),
                Err(_) => return false, // build/refresh in flight; skip this pass
            },
            None => None, // subject was never resolved; nothing to race
        };
        let removed = self.map.write().await.remove(subject).is_some();
        self.build_locks.lock().await.remove(subject);
        removed
    }

    /// Resolve the caller's own `Backend`, building it (single-flight per
    /// subject) and refreshing the token if it's near expiry. Records
    /// last-activity on every successful resolve.
    pub async fn resolve(&self, tokens: &Tokens) -> Result<Arc<Backend>, ()> {
        let subject = tokens.identity.subject.clone();
        if subject.is_empty() {
            return Err(());
        }

        // Fast path: warm, valid backend for this subject.
        if let Some(b) = self.warm(&subject, tokens).await {
            return Ok(b);
        }

        // Single-flight build/refresh per subject.
        let lock = self.subject_lock(&subject).await;
        let _guard = lock.lock().await;
        if let Some(b) = self.warm(&subject, tokens).await {
            return Ok(b);
        }

        let live = self.mint_tokens(tokens).await?; // refresh if id_token empty / near expiry

        // If an entry already exists for this subject (it was just stale/near
        // expiry, not missing), hot-swap the refreshed token into the existing
        // backend instead of rebuilding it from scratch.
        {
            let mut map = self.map.write().await;
            if let Some(entry) = map.get_mut(&subject) {
                entry.backend.set_token(&live.id_token).map_err(|_| ())?;
                entry.tokens = live;
                entry.last_active = Instant::now();
                return Ok(entry.backend.clone());
            }
        }

        let backend = self.build_backend(&live).await?; // Backend::connect_with_token
        self.insert(subject, backend.clone(), live).await;
        Ok(backend)
    }

    async fn warm(&self, subject: &str, cookie: &Tokens) -> Option<Arc<Backend>> {
        let mut map = self.map.write().await;
        let e = map.get_mut(subject)?;
        // Only reuse for the same subject (guaranteed by key) and if not near expiry.
        if e.tokens.needs_refresh() || e.tokens.id_token.is_empty() {
            return None;
        }
        // Adopt fresher identity/groups from the cookie if newer login changed
        // them: a re-login can rotate group membership (e.g. after an RBAC/IdP
        // change) while the cached entry's backend is still perfectly warm, so
        // only the stored identity is stale, not the connection. `require_auth`
        // re-seals `resolved_tokens(subject)` (this entry's `tokens`) back into
        // the browser cookie on every request, so if we didn't adopt here a
        // fresher cookie would be overwritten with the stale identity/groups it
        // just carried in, silently reverting a permission change for up to the
        // idle-eviction window.
        e.tokens.identity = cookie.identity.clone();
        // A request may carry a refresh token rotated by another replica. Adopt
        // it rather than re-sealing this process's stale generation over the
        // browser's newer cookie.
        if cookie.refresh_token.is_some() && cookie.refresh_token != e.tokens.refresh_token {
            e.tokens.refresh_token = cookie.refresh_token.clone();
        }
        e.last_active = Instant::now();
        Some(e.backend.clone())
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
    async fn mint_tokens(&self, tokens: &Tokens) -> Result<Tokens, ()> {
        if tokens.id_token.is_empty() || tokens.needs_refresh() {
            let provider = self.provider.clone().ok_or(())?;
            let rt = tokens.refresh_token.clone().ok_or(())?;
            provider.refresh(rt).await.map_err(|_| ())
        } else {
            Ok(tokens.clone())
        }
    }

    #[cfg(test)]
    async fn mint_tokens(&self, tokens: &Tokens) -> Result<Tokens, ()> {
        Ok(tokens.clone())
    }

    #[cfg(not(test))]
    async fn build_backend(&self, tokens: &Tokens) -> Result<Arc<Backend>, ()> {
        Backend::connect_with_token(&tokens.id_token, self.shared.clone())
            .await
            .map(Arc::new)
            .map_err(|_| ())
    }

    #[cfg(test)]
    async fn build_backend(&self, _tokens: &Tokens) -> Result<Arc<Backend>, ()> {
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

        assert!(Arc::ptr_eq(&a, &a_again), "same subject -> same backend");
        assert!(
            !Arc::ptr_eq(&a, &b),
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
            Arc::ptr_eq(&b, &b2),
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
        assert!(Arc::ptr_eq(&a_before, &a_after));
    }

    // Carry-in finding #1 (Task 5 review): `evict()` must drop the subject's
    // `build_locks` entry too, not just the `map` entry, or the lock map
    // grows unboundedly as subjects churn.
    #[tokio::test]
    async fn evict_also_drops_the_build_lock() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));
        let mut a1 = fake_tokens();
        a1.identity.subject = "alice".into();

        reg.resolve(&a1).await.unwrap();
        assert!(reg.build_locks.lock().await.contains_key("alice"));

        reg.evict("alice").await;
        assert!(
            !reg.build_locks.lock().await.contains_key("alice"),
            "evicting a subject must also drop its build_locks entry, not just the map entry"
        );
    }

    // Carry-in finding #2 (Task 5 review): evicting a subject whose build
    // lock is held by a concurrent in-flight `resolve` must not remove the
    // entry out from under that build (which would otherwise silently
    // resurrect it via the build's own `insert()` right after).
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

        reg.evict("alice").await;
        assert!(
            reg.get_backend("alice").await.is_some(),
            "evict must skip a subject whose build lock is currently held, not race it"
        );

        drop(guard);
        reg.evict("alice").await; // uncontended now — proceeds normally
        assert!(reg.get_backend("alice").await.is_none());
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

    // Fix #2 (final whole-branch review): a warm resolve must adopt the
    // cookie's identity/groups, not keep serving the entry's original ones.
    // Without this, a re-login that changed group membership (e.g. a
    // de-privileging RBAC/IdP change) would leave `require_auth` re-sealing
    // the stale, more-privileged groups back into the browser cookie for the
    // whole idle-eviction window, since the warm path never rebuilds the
    // backend (only a cold connect would otherwise pick up fresh groups).
    #[tokio::test]
    async fn warm_resolve_adopts_fresh_groups_from_the_cookie() {
        let shared = SharedCluster::for_test();
        let reg = BackendRegistry::new(shared, None, 100, Duration::from_secs(1200));
        let mut t = fake_tokens();
        t.identity.subject = "alice".into();
        t.identity.groups = vec!["admins".into()];

        let b_before = reg.resolve(&t).await.unwrap();

        // Simulate a re-login that changed group membership: same subject,
        // still-valid token (so this hits the warm path), different groups.
        let mut t2 = t.clone();
        t2.identity.groups = vec!["viewers".into()];
        t2.refresh_token = Some("rotated-by-peer".into());
        let b_after = reg.resolve(&t2).await.unwrap();

        assert!(
            Arc::ptr_eq(&b_before, &b_after),
            "warm resolve must reuse the same backend, only refresh identity"
        );
        let stored = reg.resolved_tokens("alice").await.unwrap();
        assert_eq!(
            stored.identity.groups,
            vec!["viewers".to_string()],
            "warm resolve must adopt the cookie's fresh groups into the stored entry"
        );
        assert_eq!(stored.refresh_token.as_deref(), Some("rotated-by-peer"));
    }
}
