//! Resource request/limit recommendations from Prometheus usage history.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use roder_k8s::{Backend, ScanSettings};

use super::bad_gateway;
use crate::server::AppState;

#[derive(Default, serde::Deserialize)]
pub struct RecommendQuery {
    /// Limit the scan to one namespace. Absent means cluster-wide, subject to
    /// the caller's own RBAC.
    #[serde(default)]
    namespace: Option<String>,
    /// Override the history window, in hours.
    #[serde(default)]
    history_hours: Option<f64>,
}

/// Bounds on `history_hours`. Below an hour there is nothing to average over;
/// above a month the subquery cost stops being worth the extra history.
const MIN_HISTORY_HOURS: f64 = 1.0;
const MAX_HISTORY_HOURS: f64 = 24.0 * 31.0;

fn settings(query: &RecommendQuery) -> Result<ScanSettings, ()> {
    let mut settings = ScanSettings::default();
    if let Some(hours) = query.history_hours {
        if !hours.is_finite() || !(MIN_HISTORY_HOURS..=MAX_HISTORY_HOURS).contains(&hours) {
            return Err(());
        }
        settings.history = std::time::Duration::from_secs_f64(hours * 3600.0);
    }
    Ok(settings)
}

/// Scan every workload the caller can list and recommend requests and limits.
///
/// Reads through the caller's own `Backend`, so the report only covers what
/// they are allowed to see. Unlike `/api/alerts` there is no shared cache: a
/// scan is a two-week query per workload, run when the user asks for it.
pub async fn recommendations(
    State(state): State<AppState>,
    Extension(backend): Extension<Arc<Backend>>,
    Query(query): Query<RecommendQuery>,
) -> Response {
    let prometheus = state.prometheus.read().await.as_ref().map(Arc::clone);
    let Some(prometheus) = prometheus else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(settings) = settings(&query) else {
        return (
            StatusCode::BAD_REQUEST,
            format!("history_hours must be between {MIN_HISTORY_HOURS} and {MAX_HISTORY_HOURS}"),
        )
            .into_response();
    };

    match backend
        .resource_scan(&prometheus, settings, query.namespace.as_deref())
        .await
    {
        Ok(scan) => Json(scan).into_response(),
        Err(error) => {
            tracing::warn!("recommendations: {error}");
            bad_gateway(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(hours: Option<f64>) -> RecommendQuery {
        RecommendQuery {
            namespace: None,
            history_hours: hours,
        }
    }

    #[test]
    fn the_default_window_is_krrs_two_weeks() {
        let settings = settings(&query(None)).expect("defaults are valid");
        assert_eq!(settings.history.as_secs(), 336 * 3600);
    }

    #[test]
    fn a_custom_window_is_accepted_within_bounds() {
        let settings = settings(&query(Some(24.0))).expect("one day is valid");
        assert_eq!(settings.history.as_secs(), 24 * 3600);
    }

    #[test]
    fn out_of_range_and_non_finite_windows_are_rejected() {
        for hours in [0.0, 0.5, 24.0 * 32.0, f64::NAN, f64::INFINITY, -1.0] {
            assert!(settings(&query(Some(hours))).is_err(), "{hours} accepted");
        }
    }
}
