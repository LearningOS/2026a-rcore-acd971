//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::config::BIG_STRIDE;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;
///A array of `TaskControlBlock` that is thread-safe
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// A stride scheduler.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    /// Add process back to ready queue, advancing its stride by its pass value
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        let mut inner = task.inner_exclusive_access();
        inner.stride = inner
            .stride
            .wrapping_add(BIG_STRIDE / inner.priority as u64);
        drop(inner);
        self.ready_queue.push_back(task);
    }
    /// Take the process with the minimal stride out of the ready queue
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        let mut min_idx: Option<usize> = None;
        let mut min_stride = u64::MAX;
        for (i, task) in self.ready_queue.iter().enumerate() {
            let stride = task.inner_exclusive_access().stride;
            if stride < min_stride {
                min_stride = stride;
                min_idx = Some(i);
            }
        }
        min_idx.and_then(|i| self.ready_queue.remove(i))
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
