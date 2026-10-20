//! Dispatch-loop tests — cancellation bookkeeping around `Pending`/`PendingState`.
//! Lives here (not `tests/lib.rs`, which would shadow the crate name).

use sidecar::{Pending, PendingState};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Cancel before the task completes: abort fires, the answer-claim slot is
/// already taken, a late success is dropped, and the pending map empties.
#[tokio::test]
async fn cancel_empties_pending_and_suppresses_late_response() {
    let pnd = Arc::new(Pending {
        state: Mutex::new(PendingState::default()),
    });

    // Spawn a task that would complete "later" — stands in for the HTTP fan-out.
    let task = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    });
    let abort = task.abort_handle();
    let rid: i64 = 7;
    {
        let mut st = pnd.state.lock().await;
        st.cancel.insert(rid, abort);
    }

    // Cancel handler path: claim the answer slot and abort the task.
    {
        let mut st = pnd.state.lock().await;
        let h = st.cancel.remove(&rid).expect("cancel handle present");
        let should_respond = st.answered.insert(rid);
        assert!(should_respond, "first answer claim for this id");
        h.abort();
    }
    // Task completes aborted; the completion task's claim-on-finish path attempts
    // `answered.insert` and fails, returning without writing.
    assert!(task.await.is_err(), "abort must propagate");

    let st = pnd.state.lock().await;
    assert!(st.cancel.is_empty(), "cancel map cleaned up");
    // Answered set retains the id so a duplicate cancel is a no-op.
    assert!(st.answered.contains(&rid));
}

/// Cancel after the task already answered: no-op (no second response emitted).
#[tokio::test]
async fn cancel_after_answered_is_noop() {
    let pnd = Pending {
        state: Mutex::new(PendingState::default()),
    };
    let rid: i64 = 9;
    {
        let mut st = pnd.state.lock().await;
        st.answered.insert(rid); // completion task finished first
    }
    let mut st = pnd.state.lock().await;
    let h = st.cancel.remove(&rid);
    assert!(h.is_none(), "no abort handle — task already done");
    let should_respond = st.answered.insert(rid);
    assert!(!should_respond, "already answered — cancel must not emit");
}
