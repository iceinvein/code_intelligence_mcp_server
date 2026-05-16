//! In-process log broadcaster.
//!
//! A `tracing` subscriber layer that forwards each formatted log event onto a
//! `tokio::sync::broadcast` channel. The HTTP server subscribes to the channel
//! and pipes events to clients over SSE at `/api/logs/stream`.
//!
//! The channel is intentionally lossy: slow subscribers see `Lagged` errors
//! and miss messages rather than back-pressuring the indexer. Log persistence
//! still happens via the existing daily file appenders.

use std::io;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing_subscriber::fmt::MakeWriter;

/// Buffer up to this many recent log lines for late subscribers. Tracing
/// writes one event per line in practice, so this is roughly "last N events".
const CHANNEL_CAPACITY: usize = 1024;

/// Handle returned by `LogBroadcaster::new`. Cheap to clone; the inner Arc
/// is shared between the tracing layer (writer) and the SSE handler
/// (subscriber).
#[derive(Clone)]
pub struct LogBroadcaster {
    tx: Arc<broadcast::Sender<String>>,
}

impl LogBroadcaster {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self { tx: Arc::new(tx) }
    }

    /// Open a new subscriber. Receivers see only messages broadcast after they
    /// subscribe.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// `MakeWriter` that hands out new `BroadcastWriter`s, one per
    /// `tracing::Event`. Wire into `tracing_subscriber::fmt::layer()`.
    pub fn make_writer(&self) -> BroadcastMakeWriter {
        BroadcastMakeWriter {
            tx: self.tx.clone(),
        }
    }
}

impl Default for LogBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct BroadcastMakeWriter {
    tx: Arc<broadcast::Sender<String>>,
}

impl<'a> MakeWriter<'a> for BroadcastMakeWriter {
    type Writer = BroadcastWriter;
    fn make_writer(&'a self) -> Self::Writer {
        BroadcastWriter {
            buf: Vec::with_capacity(256),
            tx: self.tx.clone(),
        }
    }
}

/// Per-event writer. tracing's fmt layer calls `write` zero or more times to
/// emit the formatted line, then drops the writer. We coalesce all writes
/// into one buffer and broadcast it on drop, so subscribers receive whole
/// log lines rather than partial fragments.
pub struct BroadcastWriter {
    buf: Vec<u8>,
    tx: Arc<broadcast::Sender<String>>,
}

impl io::Write for BroadcastWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for BroadcastWriter {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.buf);
        if let Ok(s) = String::from_utf8(bytes) {
            let trimmed = s.trim_end_matches(['\n', '\r']);
            if !trimmed.is_empty() {
                // Ignore send errors: a closed channel just means there are
                // no live subscribers, which is the common case for a daemon
                // running without an open dashboard.
                let _ = self.tx.send(trimmed.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn broadcaster_round_trip() {
        let b = LogBroadcaster::new();
        let mut rx = b.subscribe();

        // Write through a fresh writer; it broadcasts on drop.
        {
            let mw = b.make_writer();
            let mut w = mw.make_writer();
            w.write_all(b"hello world\n").unwrap();
        }

        let msg = rx.try_recv().expect("expected one message");
        assert_eq!(msg, "hello world");
    }

    #[test]
    fn empty_writer_does_not_broadcast() {
        let b = LogBroadcaster::new();
        let mut rx = b.subscribe();

        {
            let mw = b.make_writer();
            let _w = mw.make_writer();
        }

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn multi_writes_coalesce_into_one_message() {
        let b = LogBroadcaster::new();
        let mut rx = b.subscribe();

        {
            let mw = b.make_writer();
            let mut w = mw.make_writer();
            w.write_all(b"line ").unwrap();
            w.write_all(b"with ").unwrap();
            w.write_all(b"parts\n").unwrap();
        }

        let msg = rx.try_recv().expect("expected one message");
        assert_eq!(msg, "line with parts");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn dropped_subscribers_do_not_break_send() {
        let b = LogBroadcaster::new();
        // Subscribe and immediately drop the receiver.
        drop(b.subscribe());

        // Sending with no live receivers should still succeed (silently no-op).
        let mw = b.make_writer();
        let mut w = mw.make_writer();
        w.write_all(b"orphaned\n").unwrap();
        drop(w);
    }
}
