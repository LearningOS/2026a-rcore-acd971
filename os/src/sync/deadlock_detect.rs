//! Banker's-algorithm based deadlock detection for blocking mutexes and
//! semaphores. Mutex ids stay as-is; semaphore ids are offset by SEM_BASE.
//! All bookkeeping is isolated per process (and per thread inside a
//! process), so concurrently running tests that reuse the same resource ids
//! never interfere with each other.
//!
//! Only active for processes that called `sys_enable_deadlock_detect(1)`.

use crate::sync::UPSafeCell;
use alloc::collections::BTreeMap;
use lazy_static::*;

/// resource-id offset of the semaphore id space
pub const SEM_BASE: usize = 1 << 16;

/// one thread's resource ledger inside a process
#[derive(Default)]
struct ThreadRes {
    allocation: BTreeMap<usize, usize>,
    need: BTreeMap<usize, usize>,
}

/// one process' ledger: available pools + per-thread allocation/need
#[derive(Default)]
struct ProcState {
    available: BTreeMap<usize, usize>,
    threads: BTreeMap<usize, ThreadRes>,
}

impl ProcState {
    /// Safety check (banker's algorithm): is there a sequence in which every
    /// thread of this process can still get all the resources it needs and
    /// finish, releasing what it holds? A pending (blocked) request is kept
    /// in `need` until it is actually granted, so circular waits between
    /// threads are detected as an unsafe state.
    fn is_safe(&self) -> bool {
        let mut work = self.available.clone();
        let mut finished: BTreeMap<usize, bool> =
            self.threads.keys().map(|tid| (*tid, false)).collect();
        let mut progress = true;
        while progress {
            progress = false;
            for (tid, done) in finished.iter_mut() {
                if *done {
                    continue;
                }
                let res = &self.threads[tid];
                if res
                    .need
                    .iter()
                    .all(|(rid, n)| n <= work.get(rid).unwrap_or(&0))
                {
                    for (rid, held) in &res.allocation {
                        *work.entry(*rid).or_insert(0) += held;
                    }
                    *done = true;
                    progress = true;
                }
            }
        }
        finished.values().all(|done| *done)
    }

    /// Tentatively request one unit of `rid` for thread `tid`. Returns false
    /// (and rolls back) if granting it would lead to an unsafe state.
    fn request(&mut self, tid: usize, rid: usize) -> bool {
        let res = self.threads.entry(tid).or_default();
        *res.need.entry(rid).or_insert(0) += 1;
        if self.is_safe() {
            true
        } else {
            let res = self.threads.get_mut(&tid).unwrap();
            let n = res.need.get_mut(&rid).unwrap();
            *n -= 1;
            if *n == 0 {
                res.need.remove(&rid);
            }
            false
        }
    }

    /// One unit of `rid` has been granted to thread `tid`
    fn acquire(&mut self, tid: usize, rid: usize) {
        if let Some(res) = self.threads.get_mut(&tid) {
            *res.allocation.entry(rid).or_insert(0) += 1;
            if let Some(avail) = self.available.get_mut(&rid) {
                *avail = avail.saturating_sub(1);
            }
            if let Some(n) = res.need.get_mut(&rid) {
                *n -= 1;
                if *n == 0 {
                    res.need.remove(&rid);
                }
            }
        }
    }

    /// One unit of `rid` has been released by thread `tid`. Ignore silently
    /// when there is no matching acquisition (e.g. a thread got a deadlock
    /// report, exited while holding nothing, and its compensation up() runs).
    fn release(&mut self, tid: usize, rid: usize) {
        if let Some(res) = self.threads.get_mut(&tid) {
            if let Some(held) = res.allocation.get_mut(&rid) {
                if *held > 0 {
                    *held -= 1;
                    if *held == 0 {
                        res.allocation.remove(&rid);
                    }
                    *self.available.entry(rid).or_insert(0) += 1;
                }
            }
        }
    }
}

lazy_static! {
    static ref DETECTOR: UPSafeCell<BTreeMap<usize, ProcState>> =
        unsafe { UPSafeCell::new(BTreeMap::new()) };
}

/// Register a new resource pool of `count` units for process `pid`
/// (mutex creation: 1, semaphore creation: its initial count)
pub fn add_resource_pool(pid: usize, rid: usize, count: usize) {
    DETECTOR
        .exclusive_access()
        .entry(pid)
        .or_default()
        .available
        .insert(rid, count);
}

/// Enable or disable deadlock detection for process `pid`
pub fn set_enable(pid: usize, enabled: bool) {
    let mut d = DETECTOR.exclusive_access();
    if enabled {
        d.entry(pid).or_default();
    } else {
        d.remove(&pid);
    }
}

/// Is detection enabled for process `pid`?
pub fn is_enabled(pid: usize) -> bool {
    DETECTOR.exclusive_access().contains_key(&pid)
}

/// Thread `tid` of process `pid` requests one unit of resource `rid`.
/// Returns false (and records nothing) if this would make the state unsafe.
pub fn request_resource(pid: usize, tid: usize, rid: usize) -> bool {
    let mut d = DETECTOR.exclusive_access();
    match d.get_mut(&pid) {
        Some(state) => state.request(tid, rid),
        None => true,
    }
}

/// Thread `tid` of process `pid` has acquired one unit of resource `rid`
pub fn acquire_resource(pid: usize, tid: usize, rid: usize) {
    let mut d = DETECTOR.exclusive_access();
    if let Some(state) = d.get_mut(&pid) {
        state.acquire(tid, rid);
    }
}

/// Thread `tid` of process `pid` has released one unit of resource `rid`
pub fn release_resource(pid: usize, tid: usize, rid: usize) {
    let mut d = DETECTOR.exclusive_access();
    if let Some(state) = d.get_mut(&pid) {
        state.release(tid, rid);
    }
}
