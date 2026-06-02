use std::time::Duration;

use super::AppState;

/// Background task: refresh ID tokens before they expire so long-running watches
/// keep working, swapping each new token into the shared cluster client.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        if state.config.dev_mode {
            return;
        }
        let Some(provider) = state.provider.clone() else {
            return;
        };

        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            for (sid, refresh_token) in state.sessions.needing_refresh().await {
                match provider.refresh(refresh_token).await {
                    Ok(tokens) => {
                        if let Some(backend) = state.backend.read().await.as_ref() {
                            if let Err(e) = backend.set_token(&tokens.id_token) {
                                tracing::warn!("failed to swap refreshed token into client: {e}");
                            }
                        }
                        state.sessions.update_tokens(&sid, tokens).await;
                        tracing::debug!("refreshed session token");
                    }
                    Err(e) => {
                        tracing::warn!("token refresh failed, dropping session: {e}");
                        state.sessions.remove(&sid).await;
                    }
                }
            }
        }
    });
}
