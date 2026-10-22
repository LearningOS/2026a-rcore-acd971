//! Synchronization and interior mutability primitives

mod condvar;
mod deadlock_detect;
mod mutex;
mod semaphore;
mod up;

pub use condvar::Condvar;
pub use deadlock_detect::{
    acquire_resource, add_resource_pool, is_enabled, release_resource, request_resource, set_enable,
    SEM_BASE,
};
pub use mutex::{Mutex, MutexBlocking, MutexSpin};
pub use semaphore::Semaphore;
pub use up::UPSafeCell;
