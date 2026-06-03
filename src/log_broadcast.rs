//! In-process log broadcaster.
//!
//! A `tracing` subscriber layer that forwards each formatted log event onto a
//! `tokio::sync::broadcast` channel. The HTTP server subscribes to the channel
//! and pipes events to clients over SSE at `/api/logs/stream`.
//!
//! A fresh `tokio::sync::broadcast` receiver only sees messages sent *after* it
//! subscribes, so a dashboard that connects mid-stream would otherwise open to
//! an empty panel until the next log event fires. To avoid that, the
//! broadcaster also keeps a small in-memory ring of recent lines;
//! `subscribe_with_history` snapshots that ring and subscribes under one lock,
//! so each line appears exactly once across (history, live).
//!
//! The live channel is intentionally lossy: slow subscribers see `Lagged`
//! errors and miss messages rather than back-pressuring the indexer. Log
//! persistence still happens via the existing daily file appenders.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tracing_subscriber::fmt::MakeWriter;

/// Bounds how far a *live* subscriber may lag before it gets `Lagged`. Not a
/// replay buffer: a new `broadcast` receiver never sees the backlog (that is
/// what the history ring below is for).
const CHANNEL_CAPACITY: usize = 1024;

/// Recent log lines retained for late subscribers (the initial backfill on
/// connect). Matches the frontend's display cap so we don't ship lines that the
/// client would immediately trim.
const HISTORY_CAPACITY: usize = 500;

/// State shared between the tracing layer (writer) and SSE handlers
/// (subscribers). The history `Mutex` is held by the writer while it both
/// pushes a line and broadcasts it, and by `subscribe_with_history` while it
/// snapshots the ring and subscribes, making those two operations atomic with
/// respect to each other.
struct Shared {
    tx: broadcast::Sender<String>,
    history: Mutex<VecDeque<String>>,
}

/// Handle returned by `LogBroadcaster::new`. Cheap to clone; the inner Arc
/// is shared between the tracing layer (writer) and the SSE handler
/// (subscriber).
#[derive(Clone)]
pub struct LogBroadcaster {
    inner: Arc<Shared>,
}

impl LogBroadcaster {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            inner: Arc::new(Shared {
                tx,
                history: Mutex::new(VecDeque::with_capacity(HISTORY_CAPACITY)),
            }),
        }
    }

    /// Open a new subscriber. Receivers see only messages broadcast after they
    /// subscribe.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.inner.tx.subscribe()
    }

    /// Snapshot the recent-history ring and open a live subscriber atomically.
    /// The returned `Vec` should be replayed to the client first, then the
    /// receiver streamed; together they cover every line exactly once.
    pub fn subscribe_with_history(&self) -> (Vec<String>, broadcast::Receiver<String>) {
        // Hold the history lock across both the snapshot and the subscribe so a
        // line landing concurrently is either fully in the snapshot (and not
        // yet on the new receiver) or fully on the receiver (and not in the
        // snapshot), never both and never neither.
        let history = self
            .inner
            .history
            .lock()
            .expect("log history mutex poisoned");
        let rx = self.inner.tx.subscribe();
        let snapshot = history.iter().cloned().collect();
        (snapshot, rx)
    }

    /// `MakeWriter` that hands out new `BroadcastWriter`s, one per
    /// `tracing::Event`. Wire into `tracing_subscriber::fmt::layer()`.
    pub fn make_writer(&self) -> BroadcastMakeWriter {
        BroadcastMakeWriter {
            inner: self.inner.clone(),
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
    inner: Arc<Shared>,
}

impl<'a> MakeWriter<'a> for BroadcastMakeWriter {
    type Writer = BroadcastWriter;
    fn make_writer(&'a self) -> Self::Writer {
        BroadcastWriter {
            buf: Vec::with_capacity(256),
            inner: self.inner.clone(),
        }
    }
}

/// Per-event writer. tracing's fmt layer calls `write` zero or more times to
/// emit the formatted line, then drops the writer. We coalesce all writes
/// into one buffer and broadcast it on drop, so subscribers receive whole
/// log lines rather than partial fragments.
pub struct BroadcastWriter {
    buf: Vec<u8>,
    inner: Arc<Shared>,
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
                let line = trimmed.to_string();
                // Push to the history ring and broadcast under one lock so the
                // pair is atomic relative to `subscribe_with_history` (no
                // line lands in both the snapshot and the live stream, or in
                // neither). Keep the critical section to these two cheap ops.
                let mut history = self
                    .inner
                    .history
                    .lock()
                    .expect("log history mutex poisoned");
                if history.len() >= HISTORY_CAPACITY {
                    history.pop_front();
                }
                history.push_back(line.clone());
                // Ignore send errors: a closed channel just means there are
                // no live subscribers, which is the common case for a daemon
                // running without an open dashboard.
                let _ = self.inner.tx.send(line);
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

    /// Emit one full log line through a fresh writer (broadcasts on drop).
    fn emit(b: &LogBroadcaster, msg: &str) {
        let mw = b.make_writer();
        let mut w = mw.make_writer();
        w.write_all(msg.as_bytes()).unwrap();
        w.write_all(b"\n").unwrap();
    }

    #[test]
    fn subscribe_with_history_replays_recent_lines() {
        let b = LogBroadcaster::new();
        // Lines emitted *before* anyone subscribes must still reach a late
        // subscriber via the history snapshot. This is the empty-panel fix.
        emit(&b, "first");
        emit(&b, "second");
        emit(&b, "third");

        let (history, _rx) = b.subscribe_with_history();
        assert_eq!(history, vec!["first", "second", "third"]);
    }

    #[test]
    fn history_is_capped_at_capacity() {
        let b = LogBroadcaster::new();
        for i in 0..(HISTORY_CAPACITY + 50) {
            emit(&b, &format!("line {i}"));
        }

        let (history, _rx) = b.subscribe_with_history();
        assert_eq!(history.len(), HISTORY_CAPACITY);
        // Oldest entries dropped; newest retained, in order.
        assert_eq!(history.first().unwrap(), "line 50");
        assert_eq!(
            history.last().unwrap(),
            &format!("line {}", HISTORY_CAPACITY + 49)
        );
    }

    #[test]
    fn history_snapshot_and_live_stream_do_not_duplicate() {
        let b = LogBroadcaster::new();
        emit(&b, "historical");

        let (history, mut rx) = b.subscribe_with_history();
        assert_eq!(history, vec!["historical"]);

        // A line emitted after subscribing arrives live, and is NOT already in
        // the snapshot we took above (exactly-once across history + live).
        emit(&b, "live");
        assert_eq!(rx.try_recv().unwrap(), "live");
        assert!(rx.try_recv().is_err());
        assert!(!history.contains(&"live".to_string()));
    }
}
