# Lab 2 实验报告：地址空间（ch4）

## 实验目标

在 ch3 分时多任务的基础上引入 SV39 虚拟内存：

1. 理解并使用 RV64 的 SV39 三级页表机制，为每个任务建立独立地址空间（跳板页、TrapContext、用户栈、应用各段）。
2. 实现内核/用户地址空间的分离：trap 时通过跳板页切换 satp，内核通过页表翻译访问用户内存。
3. 实现系统调用 `sys_mmap`、`sys_munmap`（匿名映射的申请与释放），并把 ch3 的 `sys_get_time` 改造为通过页表翻译访问的实现。

## 实验原理简述

### SV39 页表与多道程序地址空间

- SV39 采用三级页表（39 位虚地址 = 27 位 VPN + 12 位偏移，VPN 分为 9/9/9 三级索引），页表项含 V/R/W/X/U/G/A/D 标志位。U 位标记用户态可访问；R=0 且 W=1 是非法组合，任何访问都会触发异常。
- ch4 中每个应用的各段（代码/数据）、用户栈、堆（sbrk 区域）都以 `MapType::Framed` 按帧分配；内核各段为 `MapType::Identical` 恒等映射，因此内核可以直接用物理地址访问所有已分配的帧。
- trap.S 中的 `__alltraps` 在进入内核时把 satp 换成内核页表；`__restore` 用 `a1` 传入的用户 satp 恢复到应用地址空间，跳板页在两个地址空间中映射到同一物理页。

### 内核如何访问用户内存

trap 进入内核后运行在内核地址空间，用户虚拟地址不能直接解引用。正确做法是用 `PageTable::from_token(current_user_token())` 构造当前任务页表的"只读视图"，逐页 `translate` 得到物理页，再访问页内偏移。`translated_byte_buffer` 已封装了跨页翻译，但它内部 `unwrap`，遇到未映射地址会 panic；`sys_get_time` 的目标地址来自用户，因此实现带 `Option` 语义与权限检查的翻译函数。

## 实现要点

### 1. sys_get_time 的页表化改造

新增辅助函数 `translated_user_page(page_table, addr, need_write)`：translate 得到 PTE 后依次校验 `V`、`U`、按方向要求 `W`/`R`，通过则返回物理页字节切片。`sys_get_time` 把 16 字节的 `TimeVal` 逐字节写回用户指针——逐字节翻译天然处理了 `TimeVal` 跨两页的情况，任一字节不可写时返回 -1。

### 2. sys_mmap / sys_munmap

`sys_mmap(start, len, port)`：

- `len == 0` 或 `start` 未按页对齐 → -1；
- `port == 0` 或含有 R/W/X（bit0/1/2）以外的位 → -1；
- 由 port 构造 `MapPermission`，恒定附加 `U` 位（用户态要直接访问）；
- 在 `MemorySet::insert_mmap_area` 中把 `[start, start+len)` 向上取整为 VPN 区间，与所有既有 area 做重叠检查（`start_vpn < area_end && end_vpn > area_start`），无冲突才以 `MapType::Framed` 插入并逐页分配帧。

`sys_munmap(start, len)`：两个参数都必须页对齐且非零；`MemorySet::remove_framed_area` 查找 VPN 区间与 `[start, start+len)` **完全一致**的 area，找到则 `unmap` 所有页（清除页表项并释放 FrameTracker）后移除，返回 0；否则 -1。

`ch4_mmap1/mmap2` 验证非法访问：R=0/W=1 的 PTE 是 RISC-V 非法组合，访问触发异常，trap_handler 按缺页处理杀死任务。

### 3. sys_sbrk

模板已提供 `change_program_brk`，通过 `append_to` / `shrink_to` 增减 `program_brk`，缩减量不能低于 `heap_bottom`，越界返回 -1。

## 测试结果

本地通过官方检查器验证 `make -C ci-user test CHAPTER=4` 全部通过（16/16），含 ch3 的 yield/sleep 用例与 ch4 的 mmap/munmap/sbrk/trace 用例。

## 思考题

1. **为什么 mmap 区域要附加 U 位？**
   U 模式只能访问带 U 位的页。mmap 返回的地址要被应用直接读写，页表项必须带 U。

2. **如果 TimeVal 指针恰好横跨两页，逐字节翻译为什么仍然正确？**
   每个字节独立按其所在页翻译：`va.floor()` 定位页，`page_offset()` 定位页内偏移。两页都有效且可写则分别写正确的物理页；任一页无效整体返回 -1。

3. **munmap 之后重新 mmap 同一区域，写入的数据还在吗？**
   不在。`remove_framed_area` 连同 `data_frames` 一起释放，物理帧归还帧分配器；重新 mmap 分配到新帧，内容不确定。
