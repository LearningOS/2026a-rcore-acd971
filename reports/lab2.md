# Lab 2 实验报告：地址空间（ch4）

## 实验目标

在 ch3 分时多任务的基础上引入 SV39 虚拟内存：

1. 理解并使用 RV64 的 SV39 三级页表机制，为每个任务建立独立地址空间（跳板页、TrapContext、用户栈、应用各段）。
2. 实现内核/用户地址空间的分离：trap 时通过跳板页切换 satp，内核通过页表翻译访问用户内存。
3. 实现系统调用 `sys_mmap`、`sys_munmap`（匿名映射的申请与释放），并把 ch3 的 `sys_trace`（读/写用户内存）、`sys_get_time`（向用户指针回写时间）改造为通过页表翻译访问的实现。

## 实验原理简述

### SV39 页表与多道程序地址空间

- SV39 采用三级页表（39 位虚地址 = 27 位 VPN + 12 位偏移，VPN 分为 9/9/9 三级索引），页表项含 V/R/W/X/U/G/A/D 标志位。U 位标记用户态可访问；R=0 且 W=1 是非法组合，任何访问都会触发异常。
- ch4 中每个应用的各段（代码/数据）、用户栈、堆（sbrk 区域）都以 `MapType::Framed` 按帧分配；内核各段为 `MapType::Identical` 恒等映射，因此内核可以直接用物理地址访问所有已分配的帧。
- trap.S 中的 `__alltraps` 在进入内核时交换 `sscratch` 并把 satp 换成内核页表；`__restore` 用 `a1` 传入的用户 satp 恢复到应用地址空间，跳板页（TRAMPOLINE）在两个地址空间中映射到同一物理页，保证切换瞬间代码可执行。

### 内核如何访问用户内存

trap 进入内核后运行在内核地址空间，用户虚拟地址不能直接解引用。正确做法是用 `PageTable::from_token(current_user_token())` 构造当前任务页表的"只读视图"，逐页 `translate` 得到物理页，再访问页内偏移。`translated_byte_buffer` 已封装了跨页翻译，但它内部 `unwrap`，遇到未映射地址会 panic；本实验的 `sys_trace` 需要探测任意地址，因此实现了自己的翻译函数并显式处理 `None`。

## 实现要点

### 1. 系统调用计数（沿用 ch3 语义）

`TaskControlBlock` 增加 `syscall_counts: [u32; MAX_SYSCALL_NUM]`，在 `syscall()` 分发入口处先加一再分发，保证 `count_syscall(SYSCALL_TRACE)` 的查询包含自身。ch4 的 TCB 已不是 `Copy` 类型，字段初始化在 `TaskControlBlock::new` 中显式清零。

### 2. sys_trace 与 sys_get_time 的页表化改造

新增辅助函数 `translated_user_page(page_table, addr, need_write)`：

- `translate(va.floor())` 得到 PTE，`None` 或 `!is_valid()` 返回 `None`；
- 校验 `PTEFlags::U`（用户态可访问的页才允许 trace 探测）；
- 按读写方向分别要求 `PTEFlags::W` / `PTEFlags::R`；
- 通过则返回物理页字节切片，按 `page_offset()` 访问单个字节。

- `sys_trace(0/1, ...)`：逐字节翻译并读/写，任何一步失败返回 -1/`None`。
- `sys_get_time`：把 16 字节的 `TimeVal` 逐字节写回用户指针。逐字节翻译天然处理了 `TimeVal` 恰好跨越两页的情况（这也是提示中的思考点），且任一字节不可写时返回 -1。

`ch4_trace1` 验证了各边界：栈上变量（U|R|W）可读可写；`isize::MAX` 未映射返回 `None`/`-1`；内核地址 `0x80200000` 不在用户页表中返回 `None`/`-1`；mmap 出来的只读页可读不可写；munmap 之后不可读。

### 3. sys_mmap / sys_munmap

`sys_mmap(start, len, port)`：

- `len == 0` 或 `start` 未按页对齐 → -1；
- `port == 0` 或含有 R/W/X（bit0/1/2）以外的位 → -1；
- 由 port 构造 `MapPermission`，恒定附加 `U` 位（用户态要直接访问）；
- 在 `MemorySet::insert_mmap_area` 中把 `[start, start+len)` 向上取整为 VPN 区间，与所有既有 area 做重叠检查（`start_vpn < area_end && end_vpn > area_start`），无冲突才以 `MapType::Framed` 插入并逐页分配帧。

`ch4_mmap3` 验证了重叠（`mmap(start-len, len+1)` 与既有页相碰）、起始未对齐、`port=0`、`port` 含非法位四种失败分支。

`sys_munmap(start, len)`：

- `len == 0`、`start` 或 `len` 未按页对齐 → -1；
- `MemorySet::remove_framed_area` 查找 VPN 区间与 `[start, start+len)` **完全一致**的 area，找到则 `unmap` 所有页（清除页表项并释放 FrameTracker）后移除，返回 0；否则 -1。

`ch4_unmap2` 验证了长度未对齐与起始未对齐都会失败；`ch4_unmap` 验证了精确区间释放后该区域可以被重新 mmap。

### 4. sys_sbrk（模板已给基础设施，验证正确性）

`TaskControlBlock::change_program_brk` 通过 `append_to` / `shrink_to` 增减 `program_brk`，缩减量不能低于 `heap_bottom`。`ch4b_sbrk` 依次申请 1 页、10 页，再一次性退还 11 页，最后再退 1 页应失败（-1），且向已释放页写入会触发缺页被内核杀死。

## 遇到的问题与解决

1. **不能复用 translated_byte_buffer 做探测**：它内部对 `translate(...).unwrap()`，而 `ch4_trace1` 要求对未映射地址返回 `None` 而不是内核 panic。于是自己实现了带 `Option` 语义与权限检查的 `translated_user_page`。
2. **W-only 页的语义**：`ch4_mmap2` 以 `port=2`（仅写）映射后读该页，期望程序被杀死。RISC-V 中 R=0 且 W=1 是非法 PTE 组合，QEMU 对任何访问都会触发异常，trap_handler 按缺页处理杀死任务，行为正好符合预期，无需特判。
3. **mmap 权限必须带 U 位**：忘加 U 位时用户态访问会直接缺页。最终把 `MapPermission::U` 作为 mmap 区域的固定成分。
4. **munmap 的匹配粒度**：采用"区间完全一致才允许释放"，与 `ch4_unmap2` 的负例一致，也避免了部分释放导致的碎片化区间管理。

## 测试结果

本地通过官方检查器验证：

```
make -C ci-user test CHAPTER=4
...
[PASS] found <Test 04_1 OK!>
[PASS] found <Test 04_4 test OK!>
[PASS] found <Test 04_5 ummap OK!>
[PASS] found <Test 04_6 ummap2 OK!>
[PASS] found <Test trace_1 OK!>
...
Report for lab1 found.
Report for lab2 found.
```

ch3 的既有用例（yield/sleep/sleep1/trace）在 ch4 内核上继续全部通过，说明地址空间改造没有破坏原有系统调用语义。

## 思考题

1. **为什么 mmap 区域要附加 U 位？**
   SV39 中 S 模式默认不能访问 U 位页（SUM 关闭时），U 模式只能访问带 U 位的页。mmap 返回的地址要被应用直接读写，因此页表项必须带 U；内核若要访问该区域，需走翻译接口（本内核在 S 态访问 U 页靠的是恒等映射的物理地址，而非用户虚地址）。

2. **如果 TimeVal 指针恰好横跨两页，逐字节翻译为什么仍然正确？**
   每个字节独立按其所在页翻译：`translated_user_page` 内部用 `va.floor()` 定位页，`page_offset()` 定位页内偏移。两页只要都有效且可写，写操作就分别落在两个正确的物理页上；任何一页无效则整体返回 -1，与 POSIX 语义（要么全成功要么不改）略有差异，但满足本实验测试要求。

3. **trace 探测内核地址为什么返回 None？**
   ch4 中内核与应用使用不同页表。用户页表里只映射了应用段、用户栈、堆、跳板页和 TrapContext，内核的代码/数据段并不在其中，`translate` 查不到 PTE，自然返回 `None`。这同时也是一种隔离：应用无法通过 sys_trace 偷看内核内存。

4. **munmap 之后重新 mmap 同一区域，写入的数据还在吗？**
   不在。`remove_framed_area` 会连同 `data_frames` 一起释放，物理帧归还给帧分配器；重新 mmap 分配到的是新帧（内容不确定）。虚拟地址可以复用，但背后是不同的物理页。
