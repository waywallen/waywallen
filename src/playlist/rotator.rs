//! Rotation handle + watch wiring for the auto-rotation task.
//!
//! The handle stores the live [`RotationConfig`] in a
//! `tokio::sync::watch` channel. The rotator task itself lives in
//! `control.rs` because it needs `AppState` + `control::step`, which
//! are private to the binary; this module is only the value types
//! and the cheap-to-clone handle so other lib code can issue updates.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::watch;

/// Live rotation parameters. `kick` is a monotonic counter the
/// rotator watches purely to reset its deadline; its value has no
/// meaning beyond "something changed since the last tick".
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct RotationConfig {
    pub interval_secs: u32,
    pub kick: u64,
}

/// Cheap-to-clone handle for sending updates into the rotator. The
/// rotator owns the matching `watch::Receiver`.
#[derive(Clone)]
pub struct RotationHandle {
    tx: watch::Sender<RotationConfig>,
    kick_counter: Arc<AtomicU64>,
}

impl RotationHandle {
    pub fn set_interval(&self, interval_secs: u32) {
        let cur = *self.tx.borrow();
        if cur.interval_secs == interval_secs {
            return;
        }
        let _ = self.tx.send(RotationConfig {
            interval_secs,
            kick: cur.kick,
        });
    }

    pub fn interval(&self) -> u32 {
        self.tx.borrow().interval_secs
    }

    /// Reset the rotator's deadline. Idempotent. Cheap.
    pub fn kick(&self) {
        let next = self.kick_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let cur = *self.tx.borrow();
        let _ = self.tx.send(RotationConfig {
            interval_secs: cur.interval_secs,
            kick: next,
        });
    }
}

/// Construct the handle. The matching receiver is what the rotator
/// task in `control` consumes.
pub fn make_handle() -> (RotationHandle, watch::Receiver<RotationConfig>) {
    let (tx, rx) = watch::channel(RotationConfig::default());
    let handle = RotationHandle {
        tx,
        kick_counter: Arc::new(AtomicU64::new(0)),
    };
    (handle, rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn set_interval_dedups_no_op_writes() {
        let (handle, mut rx) = make_handle();
        // Initial config — observed once on subscribe.
        let initial = *rx.borrow_and_update();
        assert_eq!(initial.interval_secs, 0);

        handle.set_interval(0); // no change → no event
        assert!(
            tokio::time::timeout(Duration::from_millis(20), rx.changed())
                .await
                .is_err()
        );

        handle.set_interval(60); // real change → event
        assert!(rx.changed().await.is_ok());
        assert_eq!(rx.borrow().interval_secs, 60);
    }

    #[tokio::test]
    async fn kick_changes_config_each_call() {
        let (handle, mut rx) = make_handle();
        let _ = rx.borrow_and_update();
        let before = *rx.borrow();

        handle.kick();
        rx.changed().await.unwrap();
        let after = *rx.borrow();
        assert!(after.kick > before.kick);

        handle.kick();
        rx.changed().await.unwrap();
        assert!(rx.borrow().kick > after.kick);
    }
}
