#![recursion_limit = "512"]

// Server binary uses jemalloc: much lower RSS and fragmentation than glibc
// malloc for roder's long-lived, watch-heavy allocation pattern. Behind the
// `jemalloc` feature (enabled only in the production image) so local NixOS dev
// builds — where jemalloc's C source won't compile — are unaffected.
#[cfg(all(not(target_arch = "wasm32"), feature = "jemalloc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    use axum::middleware::from_fn_with_state;
    use axum::routing::{get, post};
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use roder::app::{shell, App};
    use roder::server::{api, build_state, handlers};

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,roder=debug".into()),
        )
        .init();

    let conf = get_configuration(None).unwrap();
    let addr = conf.leptos_options.site_addr;
    let leptos_options = conf.leptos_options.clone();
    let routes = generate_route_list(App);

    let state = match build_state(conf.leptos_options).await {
        Ok(state) => state,
        Err(e) => {
            tracing::error!("startup failed: {e}");
            std::process::exit(1);
        }
    };

    // App pages + private API, gated behind a valid session.
    let protected = Router::new()
        .route("/api/me", get(handlers::me))
        .route("/api/overview", get(api::overview))
        .route("/api/features", get(api::features))
        .route("/api/talos/node", get(api::talos_node))
        .route("/api/talos/dmesg", get(api::talos_dmesg))
        .route("/api/resources", get(api::resources))
        .route("/api/namespaces", get(api::namespaces))
        .route("/api/detail", get(api::detail))
        .route("/api/resource-tree", get(api::resource_tree))
        .route("/api/watch", get(api::watch))
        .route("/api/watch-multi", get(api::watch_multi))
        .route("/api/permissions", get(api::permissions))
        .route("/api/access-review", get(api::access_review))
        .route("/api/logs", get(api::logs))
        .route("/api/metrics", get(api::metrics_history))
        .route("/api/alerts", get(api::alerts))
        .route("/api/action", post(api::action))
        .route("/api/exec", get(api::exec_ws))
        .route("/api/debug-shell", get(api::debug_shell))
        .route("/api/node-shell", get(api::node_shell_create))
        .route("/terminal", get(api::terminal_page))
        .leptos_routes(&state, routes, {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
        .route_layer(from_fn_with_state(state.clone(), handlers::require_auth));

    let app = Router::new()
        // Public endpoints.
        .route("/health", get(handlers::health))
        .route("/auth/login", get(handlers::login))
        .route("/auth/callback", get(handlers::callback))
        .route("/auth/logout", get(handlers::logout))
        .merge(protected)
        // Static assets (incl. the wasm/css the login page needs) + SSR 404.
        .fallback(leptos_axum::file_and_error_handler::<
            roder::server::AppState,
            _,
        >(shell))
        .layer(from_fn_with_state(
            state.clone(),
            handlers::security_headers,
        ))
        .with_state(state.clone());

    log!("roder listening on http://{addr}");

    // Shutdown coordination: when SIGTERM arrives (K8s pod termination), cancel
    // all background tasks (informers, metrics refresh, CRD watch) and drain
    // in-flight HTTP requests gracefully instead of waiting for SIGKILL.
    // Required because this binary is PID 1 in the container — Linux ignores
    // SIGTERM for PID 1 unless an explicit handler is registered.
    let shutdown = async move {
        let sigterm = tokio::signal::unix::SignalKind::terminate();
        tokio::signal::unix::signal(sigterm).unwrap().recv().await;
        tracing::info!("SIGTERM received — shutting down");
        // Drop the backend (informers + watchers) so long-lived SSE connections
        // can drain; the runtime will cancel remaining tasks on `main` exit.
        state.backend.write().await.take();
    };

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown)
        .await
        .unwrap();
}

// When building the wasm (hydrate) binary target, `main` is a no-op: the entry
// point is the `hydrate()` function exported from the library.
#[cfg(not(feature = "ssr"))]
fn main() {}
