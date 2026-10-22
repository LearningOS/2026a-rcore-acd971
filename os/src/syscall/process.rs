//! Process management syscalls
use crate::{
    task::{exit_current_and_run_next, suspend_current_and_run_next},
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    unsafe {
        *ts = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    }
    0
}

/// the address range accessible to sys_trace in ch3:
/// statically loaded user apps and the per-app user stacks
fn valid_user_addr(addr: usize) -> bool {
    let base = crate::config::APP_BASE_ADDRESS;
    let end = base + crate::config::MAX_APP_NUM * crate::config::APP_SIZE_LIMIT;
    if addr >= base && addr + 1 < end {
        return true;
    }
    let (stack_lo, stack_hi) = crate::loader::user_stack_range();
    addr >= stack_lo && addr + 1 < stack_hi
}

/// investigate task status: read/write a byte in the task's memory,
/// or query how many times the task has issued a syscall
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    match trace_request {
        // read one byte at the address `id` of the current task
        0 => {
            if !valid_user_addr(id) {
                return -1;
            }
            unsafe {
                let ans = *(id as *const u8) as isize;
                trace!("kernel: sys_trace read 0x{:x} -> {}", id, ans);
                ans
            }
        }
        // write one byte `data` to the address `id` of the current task
        1 => {
            if !valid_user_addr(id) {
                return -1;
            }
            unsafe {
                *(id as *mut u8) = data as u8;
                trace!("kernel: sys_trace write 0x{:x} <- {}", id, data as u8);
            }
            0
        }
        // report how many times the current task has issued syscall `id`
        2 => crate::task::get_current_syscall_count(id) as isize,
        _ => {
            warn!("kernel: unsupported sys_trace request {}", trace_request);
            -1
        }
    }
}
