#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::middleware::{from_fn, from_fn_with_state};
    use axum::routing::{get, post};
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use roder::app::{shell, App};
    use roder::server::{api, build_state, handlers, refresh};

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

    // Keep tokens fresh so long-running watches survive.
    refresh::spawn(state.clone());

    // App pages + private API, gated behind a valid session.
    let protected = Router::new()
        .route("/api/me", get(handlers::me))
        .route("/api/overview", get(api::overview))
        .route("/api/resources", get(api::resources))
        .route("/api/namespaces", get(api::namespaces))
        .route("/api/detail", get(api::detail))
        .route("/api/watch", get(api::watch))
        .route("/api/permissions", get(api::permissions))
        .route("/api/logs", get(api::logs))
        .route("/api/metrics", get(api::metrics_history))
        .route("/api/action", post(api::action))
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
        .fallback(leptos_axum::file_and_error_handler::<roder::server::AppState, _>(shell))
        .layer(from_fn(handlers::security_headers))
        .with_state(state);

    log!("roder listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

// When building the wasm (hydrate) binary target, `main` is a no-op: the entry
// point is the `hydrate()` function exported from the library.
#[cfg(not(feature = "ssr"))]
fn main() {}
