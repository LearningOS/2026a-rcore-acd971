//! Process management syscalls
use crate::config::PAGE_SIZE;
use crate::mm::{MapPermission, PageTable, PTEFlags, VirtAddr};
use crate::task::{
    change_program_brk, current_user_token, exit_current_and_run_next,
    get_current_syscall_count, insert_current_mmap_area, remove_current_mmap_area,
    suspend_current_and_run_next,
};
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// Translate a user virtual address through the current user page table and
/// check permission bits. Returns the 4KiB physical page bytes if the page is
/// valid, user-accessible and matches the required access, otherwise `None`.
fn translated_user_page(page_table: &PageTable, addr: usize, need_write: bool) -> Option<&'static mut [u8]> {
    let va = VirtAddr::from(addr);
    let pte = page_table.translate(va.floor())?;
    if !pte.is_valid() {
        return None;
    }
    let flags = pte.flags();
    if !flags.contains(PTEFlags::U) {
        return None;
    }
    if need_write && !flags.contains(PTEFlags::W) {
        return None;
    }
    if !need_write && !flags.contains(PTEFlags::R) {
        return None;
    }
    Some(pte.ppn().get_bytes_array())
}

/// Read one byte at user address `addr` through the page table.
fn user_read_byte(page_table: &PageTable, addr: usize) -> Option<u8> {
    let page = translated_user_page(page_table, addr, false)?;
    Some(page[VirtAddr::from(addr).page_offset()])
}

/// Write one byte `val` to user address `addr` through the page table.
fn user_write_byte(page_table: &PageTable, addr: usize, val: u8) -> Option<()> {
    let page = translated_user_page(page_table, addr, true)?;
    page[VirtAddr::from(addr).page_offset()] = val;
    Some(())
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond, written back through the user page table
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    let tv = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let src = &tv as *const TimeVal as *const u8;
    let page_table = PageTable::from_token(current_user_token());
    for i in 0..core::mem::size_of::<TimeVal>() {
        let byte = unsafe { src.add(i).read() };
        if user_write_byte(&page_table, ts as usize + i, byte).is_none() {
            return -1;
        }
    }
    0
}

/// investigate task status: read/write a byte in the task's address space
/// (translated through the user page table with permission checks), or query
/// how many times the task has issued a syscall
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    match trace_request {
        // read one byte at the address `id` of the current task
        0 => {
            let page_table = PageTable::from_token(current_user_token());
            match user_read_byte(&page_table, id) {
                Some(byte) => byte as isize,
                None => -1,
            }
        }
        // write one byte `data` to the address `id` of the current task
        1 => {
            let page_table = PageTable::from_token(current_user_token());
            match user_write_byte(&page_table, id, data as u8) {
                Some(()) => 0,
                None => -1,
            }
        }
        // report how many times the current task has issued syscall `id`
        2 => get_current_syscall_count(id) as isize,
        _ => {
            warn!("kernel: unsupported sys_trace request {}", trace_request);
            -1
        }
    }
}

/// map memory at the page-aligned `start` with length `len` and permissions
/// from `port` (bit0 R, bit1 W, bit2 X); fails on unaligned args, invalid
/// port bits, or overlapping an existing area
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel: sys_mmap");
    if len == 0 || start % PAGE_SIZE != 0 {
        return -1;
    }
    if port == 0 || port & !0b111 != 0 {
        return -1;
    }
    let mut perm = MapPermission::U;
    if port & 0b001 != 0 {
        perm |= MapPermission::R;
    }
    if port & 0b010 != 0 {
        perm |= MapPermission::W;
    }
    if port & 0b100 != 0 {
        perm |= MapPermission::X;
    }
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(start + len);
    if insert_current_mmap_area(start_va, end_va, perm) {
        0
    } else {
        -1
    }
}

/// unmap the memory range [start, start+len); both args must be page-aligned
/// and the range must exactly match an existing mapped area
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap");
    if len == 0 || start % PAGE_SIZE != 0 || len % PAGE_SIZE != 0 {
        return -1;
    }
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(start + len);
    if remove_current_mmap_area(start_va, end_va) {
        0
    } else {
        -1
    }
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
