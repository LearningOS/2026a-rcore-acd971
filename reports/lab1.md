# Lab 1 实验报告：多道程序与分时多任务（ch3）

## 实验目标

在 ch2 批处理系统的基础上，将内核改造为支持多道程序与分时多任务的操作系统：

1. 多个应用同时驻留内存，由内核统一调度，一个应用退出后自动切换到下一个就绪应用，而不是一次只加载一个应用。
2. 实现 `__switch` 汇编例程完成任务上下文切换，配合时钟中断实现时间片轮转。
3. 实现新的系统调用 `sys_trace`（syscall id 410），支持读取/写入当前应用内存中的一个字节，以及查询当前应用发起某系统调用的次数。

## 实验原理简述

### 任务上下文与切换

每个任务（TaskControlBlock，TCB）保存自己的状态 `TaskStatus` 与 `TaskContext`。`TaskContext` 只包含被调用者保存（callee-saved）寄存器：`ra`、`sp`、`s0`～`s11`。切换由 `__switch` 完成：

- 保存当前任务的 callee-saved 寄存器到 `current_task_cx_ptr`；
- 从 `next_task_cx_ptr` 恢复下一个任务的寄存器；
- `ret` 到下一任务的 `ra`，从而"跳"到对方上次挂起的位置继续执行。

由于 trap 流程已经把所有调用者保存寄存器保存在内核栈的 TrapContext 里，`__switch` 只需处理 callee-saved 部分。

### 多道程序加载与调度

- `loader.rs` 不再把应用逐个覆盖加载到 `APP_BASE_ADDRESS`，而是为每个应用分配独立地址区间 `APP_BASE_ADDRESS + i * APP_SIZE_LIMIT`，内核栈也按应用独立分配，并用 `init_app_cx` 在各内核栈上伪造好第一次进入用户态的 TrapContext，再由 `TaskContext::goto_restore` 包装成任务上下文。
- `TaskManager` 维护 `tasks` 数组与 `current_task`。调度采用 round-robin：`find_next_task` 从当前任务开始环形查找第一个 `Ready` 的任务；`suspend_current_and_run_next`（yield/时间片用尽）把当前任务置回 `Ready`，`exit_current_and_run_next` 置为 `Exited`，然后 `run_next_task` 完成切换。所有应用退出后内核报 `All applications completed!` 并关机。
- 时钟中断处理中调用 `suspend_current_and_run_next()`，实现分时多任务：即使应用不主动 yield，每个时间片到期也会被切换。

### 本实验实现的系统调用 sys_trace

`sys_trace(request, id, data)` 三种请求：

| request | 语义 | 返回值 |
| --- | --- | --- |
| 0 | 读取当前应用地址空间中 `id` 地址处的一个字节 | 字节值；地址越界返回 -1 |
| 1 | 向当前应用地址空间 `id` 地址写入一个字节 `data` | 成功 0；地址越界返回 -1 |
| 2 | 查询当前应用发起系统调用 `id` 的次数（含本次 `sys_trace` 调用自身） | 次数 |

### 实现要点

1. **系统调用计数**：在 `os/src/config.rs` 增加 `MAX_SYSCALL_NUM = 500`；`TaskControlBlock` 增加 `syscall_counts: [u32; MAX_SYSCALL_NUM]`，随 TCB 初始化清零。`os/src/syscall/mod.rs` 的 `syscall()` 分发入口处，先对当前任务的 `syscall_counts[syscall_id]` 加一，再分发。这样：
   - 计数在进入具体处理函数之前完成，`request == 2` 查询时天然包含本次 `sys_trace` 调用（测试用例 `ch3_trace` 中 `assert_eq!(2, count_syscall(SYSCALL_TRACE))` 依赖这一点）；
   - 计数按任务隔离，任务退出后其计数随 TCB 一起失效，不会泄漏到其他任务。
2. **sys_trace 的读写实现**：ch3 没有页表，内核与应用共享同一地址空间，因此内核直接解引用用户地址即可。为避免野指针访问导致内核崩溃，实现中先做地址合法性检查：仅允许访问静态加载的应用区域 `[APP_BASE_ADDRESS, APP_BASE_ADDRESS + MAX_APP_NUM * APP_SIZE_LIMIT)`，越界返回 -1。
3. **任务管理函数**：`os/src/task/mod.rs` 为 `TaskManager` 增加计数相关的读写方法，通过 `UPSafeCell` 短暂借用 `inner` 修改/读取当前任务的计数，并暴露同名模块级封装函数供 `syscall` 模块调用。

### 测试结果

本地通过官方检查器验证 `make -C ci-user test CHAPTER=3` 全部通过（6/6），yield/sleep/sleep1/trace 各用例输出均符合预期。

## 思考题

1. **为什么 `__switch` 只保存/恢复 callee-saved 寄存器？**
   `__switch` 是普通函数调用，遵循 RISC-V 调用约定：调用者保存寄存器由编译器在调用点负责保存；`ra`、`sp`、`s0..s11` 属于被调用者保存，必须由 `__switch` 本身保存。用户态寄存器则已在 trap 进入内核时保存到 TrapContext。

2. **任务切换时两个栈如何配合？**
   每个任务有独立内核栈。`__switch` 只把 `sp` 在两个任务的内核栈之间切换；TrapContext 留在各自内核栈顶，恢复到用户态时由 `trap_return` 从当前内核栈上恢复。

3. **为什么 sys_trace 的计数要包含本次调用？**
   这是接口语义的一部分：应用需要知道"到目前为止一共发起了多少次 trace"，包含自身才满足测试断言 `assert_eq!(2, count_syscall(SYSCALL_TRACE))`。

4. **ch3 的 sys_trace 读写实现方式在 ch4 之后还成立吗？**
   不成立。ch4 引入地址空间后，用户地址需要通过当前任务的页表翻译，内核必须使用页表翻译接口访问用户内存，且要校验页表项的读写权限。
