//! lease（DESIGN-runtime §4・CD-19）: 同じ window / repo / CLI / browser profile に同時に 2 つの run が触らない。
//! key は Ord の安定順に並べ、全部を 1 つの lock の下で一度に取る（hold-and-wait が無いので順に依らず deadlock しない）。
//! 取れない間は拒否せず待つ（DESIGN-runtime Q3 の Queued）。待つ間も cancel flag を見る（RUN-008）。
//! 返すのは最後の持ち手が drop した時: executor は run と handler の thread で分けて持つので、時間切れで手を離した thread が
//! まだ対象を触っている間は次の run に渡らない。
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::lock;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LeaseKey {
    BrowserProfile(String),
    Cli(String),
    Repo(String),
    Window(String),
}

impl fmt::Display for LeaseKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BrowserProfile(id) => write!(f, "browser_profile:{id}"),
            Self::Cli(id) => write!(f, "cli:{id}"),
            Self::Repo(id) => write!(f, "repo:{id}"),
            Self::Window(id) => write!(f, "window:{id}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseWait {
    Cancelled,
    TimedOut,
}

/// key → 持っている run id。
#[derive(Default)]
pub struct LeaseTable {
    held: Mutex<HashMap<LeaseKey, String>>,
    freed: Condvar,
}

/// drop で返す（Arc に入れて分けて持てる）。
pub struct LeaseGuard {
    table: Arc<LeaseTable>,
    keys: Vec<LeaseKey>,
}

impl LeaseTable {
    /// 待たずに取る。誰かが 1 つでも持っていれば何も取らずに Err(その run id)。`keys` は sort / dedup 済みで渡す。
    pub fn try_acquire(self: &Arc<Self>, keys: &[LeaseKey], run_id: &str) -> Result<LeaseGuard, String> {
        let mut held = lock(&self.held);
        take(&mut held, keys, run_id).map(|()| LeaseGuard { table: self.clone(), keys: keys.to_vec() })
    }

    /// 空くまで待つ。cancel flag と deadline を 20ms ごとに見る。
    pub fn acquire(self: &Arc<Self>, keys: &[LeaseKey], run_id: &str, cancel: &AtomicBool, deadline: Instant) -> Result<LeaseGuard, LeaseWait> {
        let mut held = lock(&self.held);
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(LeaseWait::Cancelled);
            }
            if take(&mut held, keys, run_id).is_ok() {
                return Ok(LeaseGuard { table: self.clone(), keys: keys.to_vec() });
            }
            if Instant::now() >= deadline {
                return Err(LeaseWait::TimedOut);
            }
            held = self.freed.wait_timeout(held, Duration::from_millis(20)).map_or_else(|e| PoisonError::into_inner(e).0, |(g, _)| g);
        }
    }
}

/// 全部空いていれば全部取る。1 つでも塞がっていれば何も取らない。
fn take(held: &mut HashMap<LeaseKey, String>, keys: &[LeaseKey], run_id: &str) -> Result<(), String> {
    if let Some(owner) = keys.iter().find_map(|k| held.get(k)) {
        return Err(owner.clone());
    }
    for k in keys {
        held.insert(k.clone(), run_id.into());
    }
    Ok(())
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let mut held = lock(&self.table.held);
        for k in &self.keys {
            held.remove(k);
        }
        self.table.freed.notify_all();
    }
}
