use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, error, trace};

use crate::client::DebugClient;
use crate::types::events::DebugEvent;

pub struct EventListener {
    stop: Arc<AtomicBool>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl EventListener {
    pub fn start(
        client: Arc<DebugClient>,
        poll_interval_ms: u64,
    ) -> (Self, mpsc::UnboundedReceiver<DebugEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let last_error = Arc::new(Mutex::new(None::<String>));
        let last_error_clone = last_error.clone();

        std::thread::spawn(move || {
            let interval = Duration::from_millis(poll_interval_ms);

            while !stop_clone.load(Ordering::Relaxed) {
                match client.ping() {
                    Ok(events) => {
                        *last_error_clone.lock().unwrap() = None;
                        if events.is_empty() {
                            trace!("ping: no events");
                        }
                        for event in events {
                            debug!(?event, "debug event received");
                            if tx.send(event).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let text = e.to_string();
                        let mut slot = last_error_clone.lock().unwrap();
                        // The channel is polled every few milliseconds; one log line per change
                        // of the failure keeps a dead registration from flooding the log.
                        if slot.as_deref() != Some(text.as_str()) {
                            error!(%e, "ping failed");
                        }
                        *slot = Some(text);
                    }
                }

                std::thread::sleep(interval);
            }
        });

        (Self { stop, last_error }, rx)
    }

    /// Why the last poll of the event channel failed, while it keeps failing. A debugger the
    /// server no longer considers registered gets no events at all, and without this a wait for
    /// a stop could only end in a timeout.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for EventListener {
    fn drop(&mut self) {
        self.stop();
    }
}
