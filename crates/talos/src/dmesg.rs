//! Streaming a node's kernel ring buffer: an initial bounded snapshot,
//! followed by a live tail.

use std::collections::VecDeque;
use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use talos_api_rs::api::machine::machine_service_client::MachineServiceClient;
use talos_api_rs::api::machine::DmesgRequest;

use crate::error::TalosError;
use crate::request::{metadata_failure, targeted_unbounded, targeted_with_timeout};
use crate::Backend;

const DMESG_TIMEOUT: Duration = Duration::from_secs(15);
const DMESG_INITIAL_LINES: usize = 500;
const DMESG_MAX_LINE_BYTES: usize = 1024 * 1024;

impl Backend {
    pub async fn dmesg(
        &self,
        node: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, TalosError>> + Send>>, TalosError> {
        let channel = self.current_channel().await?;
        let snapshot_request = targeted_with_timeout(
            node,
            DmesgRequest {
                follow: false,
                tail: false,
            },
            DMESG_TIMEOUT,
        )?;
        let mut snapshot_stream = MachineServiceClient::new(channel.clone())
            .dmesg(snapshot_request)
            .await?
            .into_inner();
        let initial = tokio::time::timeout(DMESG_TIMEOUT, async {
            let mut lines = VecDeque::with_capacity(DMESG_INITIAL_LINES);
            while let Some(chunk) = snapshot_stream.message().await? {
                if let Some(error) = metadata_failure(std::iter::once(chunk.metadata.as_ref())) {
                    return Err(TalosError::Upstream(error));
                }
                if lines.len() == DMESG_INITIAL_LINES {
                    lines.pop_front();
                }
                lines.push_back(dmesg_line(&chunk.bytes));
            }
            Ok::<_, TalosError>(lines)
        })
        .await
        .map_err(|_| TalosError::Timeout("dmesg snapshot".into()))??;

        let follow_request = targeted_unbounded(
            node,
            DmesgRequest {
                follow: true,
                tail: true,
            },
        )?;

        Ok(Box::pin(async_stream::try_stream! {
            for line in initial {
                yield line;
            }
            let mut stream = MachineServiceClient::new(channel)
                .dmesg(follow_request)
                .await?
                .into_inner();
            while let Some(chunk) = stream.message().await? {
                if let Some(error) = metadata_failure(std::iter::once(chunk.metadata.as_ref())) {
                    Err(TalosError::Upstream(error))?;
                }
                yield dmesg_line(&chunk.bytes);
            }
        }))
    }
}

fn dmesg_line(bytes: &[u8]) -> String {
    let truncated = bytes.len() > DMESG_MAX_LINE_BYTES;
    let bytes = &bytes[..bytes.len().min(DMESG_MAX_LINE_BYTES)];
    let line = String::from_utf8_lossy(bytes)
        .trim_end_matches(&['\r', '\n'][..])
        .to_string();
    if truncated {
        format!("{line} [line truncated]")
    } else {
        line
    }
}
