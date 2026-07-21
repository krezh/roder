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
const DMESG_INITIAL_BYTES: usize = 2 * 1024 * 1024;
const DMESG_MAX_LINE_BYTES: usize = 64 * 1024;

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
            let mut lines = DmesgLines::new(
                DMESG_INITIAL_LINES,
                DMESG_INITIAL_BYTES,
                DMESG_MAX_LINE_BYTES,
            );
            while let Some(chunk) = snapshot_stream.message().await? {
                if let Some(error) = metadata_failure(std::iter::once(chunk.metadata.as_ref())) {
                    return Err(TalosError::Upstream(error));
                }
                lines.push_record(&chunk.bytes);
            }
            lines.finish_pending();
            Ok::<_, TalosError>(lines.take_completed())
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
            let mut lines = DmesgLines::new(
                DMESG_INITIAL_LINES,
                DMESG_INITIAL_BYTES,
                DMESG_MAX_LINE_BYTES,
            );
            while let Some(chunk) = stream.message().await? {
                if let Some(error) = metadata_failure(std::iter::once(chunk.metadata.as_ref())) {
                    Err(TalosError::Upstream(error))?;
                }
                lines.push_record(&chunk.bytes);
                for line in lines.take_completed() {
                    yield line;
                }
            }
            lines.finish_pending();
            for line in lines.take_completed() {
                yield line;
            }
        }))
    }
}

struct DmesgLines {
    lines: VecDeque<String>,
    retained_bytes: usize,
    pending: Vec<u8>,
    pending_truncated: bool,
    max_lines: usize,
    max_bytes: usize,
    max_line_bytes: usize,
}

impl DmesgLines {
    fn new(max_lines: usize, max_bytes: usize, max_line_bytes: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(max_lines),
            retained_bytes: 0,
            pending: Vec::with_capacity(max_line_bytes.min(4096)),
            pending_truncated: false,
            max_lines,
            max_bytes,
            max_line_bytes,
        }
    }

    fn push_record(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte == b'\n' {
                self.finish_pending();
            } else if self.pending.len() < self.max_line_bytes {
                self.pending.push(byte);
            } else {
                self.pending_truncated = true;
            }
        }
        // Talos sends each kernel record as its own gRPC message. Embedded
        // newlines still split into multiple output lines, while the message
        // boundary terminates a record that has no trailing newline.
        self.finish_pending();
    }

    fn finish_pending(&mut self) {
        if self.pending.is_empty() && !self.pending_truncated {
            return;
        }
        if self.pending.last() == Some(&b'\r') {
            self.pending.pop();
        }
        let mut line = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        if std::mem::take(&mut self.pending_truncated) {
            line.push_str(" [line truncated]");
        }
        self.retained_bytes += line.len();
        self.lines.push_back(line);
        while self.lines.len() > self.max_lines || self.retained_bytes > self.max_bytes {
            if let Some(removed) = self.lines.pop_front() {
                self.retained_bytes = self.retained_bytes.saturating_sub(removed.len());
            } else {
                break;
            }
        }
    }

    fn take_completed(&mut self) -> VecDeque<String> {
        self.retained_bytes = 0;
        std::mem::take(&mut self.lines)
    }
}

#[cfg(test)]
mod tests {
    use super::DmesgLines;

    #[test]
    fn records_without_newlines_are_emitted_individually() {
        let mut lines = DmesgLines::new(10, 100, 20);
        lines.push_record(b"first");
        lines.push_record(b"second");
        assert_eq!(
            lines.take_completed().into_iter().collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn retained_snapshot_is_bounded_by_lines_and_bytes() {
        let mut lines = DmesgLines::new(3, 8, 20);
        lines.push_record(b"one\ntwo\nthree\nfour");
        let retained = lines.take_completed();
        assert!(retained.len() <= 3);
        assert!(retained.iter().map(String::len).sum::<usize>() <= 8);
        assert_eq!(retained.back().map(String::as_str), Some("four"));
    }

    #[test]
    fn oversized_line_is_truncated_without_growing_pending_buffer() {
        let mut lines = DmesgLines::new(10, 100, 4);
        lines.push_record(b"abcdefgh");
        assert_eq!(lines.pending.capacity(), 4);
        assert_eq!(
            lines.take_completed().pop_front().as_deref(),
            Some("abcd [line truncated]")
        );
    }
}
