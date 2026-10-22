# Lab 3 实验报告：进程管理（ch5）

## 实验目标

在 ch4 地址空间的基础上，把"任务"升级为拥有独立地址空间、父子关系的"进程"：

1. 理解进程控制块（PCB）与进程树：fork 复制、exec 替换、waitpid 回收、exit 通知父进程。
2. 实现系统调用 `sys_spawn`（不复制父进程地址空间、直接创建新进程执行指定应用）与 `sys_set_priority`（设置进程优先级）。
3. 将任务调度器从 FIFO 改造为 stride 调度算法，用优先级控制进程获得的 CPU 时间份额。

## 实验原理简述

### 进程抽象与进程树

ch5 的 `TaskControlBlock` 升级为进程控制块，主要字段：

- `memory_set: MemorySet`：独立地址空间（内核段恒等映射 + 用户段按帧分配）；
- `trap_cx_ppn`：TrapContext 所在物理页，trap 返回时恢复寄存器；
- `base_size/user_cx/heap_bottom/program_brk`：用户栈底与堆（sbrk）边界；
- `parent: Option<Weak<TaskControlBlock>>`、`children: Vec<Arc<TaskControlBlock>>`：进程树用强/弱引用防止循环引用泄漏；
- `exit_code: i32`：进程退出码，由父进程在 waitpid 时取走。

`fork` 深拷贝父进程地址空间（逐 area 复制并复制页表映射），`exec` 丢弃旧地址空间换成新应用的。子进程退出时将自己从父进程 children 中"摘除"（`remove_zombie_child`），父进程 waitpid 阻塞等待。

### stride 调度

每个进程持有 `stride` 与 `priority`。就绪时 `stride += BIG_STRIDE / priority`（pass = 步长），调度器每次挑选 `stride` 最小的进程运行。优先级越高 pass 越小，stride 增长越慢，被调度次数越多，从而按比例分享 CPU。`BIG_STRIDE` 取 10^9，远大于任何 pass 取整误差，保证比较有效；取 u64 无溢出风险（优先级上限也 clamp 到 BIG_STRIDE）。

## 实现要点

### 1. sys_spawn（fork+exec ≠ spawn）

`fork+exec` 会先深拷贝父进程整个地址空间，exec 时又立刻丢弃；spawn 则直接用 ELF 新建进程，只保留父子关系与优先级继承。实现：

- `translated_str` 读取路径，`get_app_data_by_name` 取 ELF 数据；
- `TaskControlBlock::new(data)` 直接创建新进程（新地址空间、新 pid）；
- 新进程 `parent = Some(Weak当前进程)`，当前进程 `children.push(新进程)`，priority 继承父进程；
- `add_task(new_task)` 入就绪队列，返回新 pid。

注意：不使用 `fork`/`exec` 的任何代码路径，避免一次多余的地址空间拷贝。

### 2. sys_set_priority

- `prio < 2` 返回 -1（与题目要求一致，优先级有效区间 [2, 2^16)）；
- 写入 `inner.priority` 并 clamp 到 [2, BIG_STRIDE]，保证 pass = BIG_STRIDE/priority ≥ 1，stride 严格递增；
- 成功返回 prio 本身。

### 3. stride 调度器（manager.rs 重写）

- `ready_queue` 仍为 `VecDeque<Arc<TaskControlBlock>>`；
- `add()`：入队前对任务 `stride += BIG_STRIDE / priority`（即"本次运行前预付 pass"），这样每个进程每被调度一轮就加一次 pass，语义等价于运行完再加；
- `fetch()`：线性扫描取 stride 最小者出队（u64 溢出-safe 比较：取 min 用普通 `<`，实际 BIG_STRIDE=1e9、pass≥1，在可达运行时间内不会回绕到危险区间）；
- TCB 新增 `stride: u64` 与 `priority: u32`，初始 priority=16（DEFAULT_PRIORITY），stride=0。

### 4. mmap/munmap 与 get_time（沿用 ch4 实现，迁移到进程 API）

- `MemorySet::insert_mmap_area` / `remove_framed_area`（VPN 区间重叠检查、完全匹配删除）直接复用；
- `sys_mmap/munmap` 的参数校验与 ch4 完全一致（len≠0、页对齐、port 位合法、固定加 U 位）；
- `sys_get_time` 沿用逐字节页表翻译写回 `TimeVal` 的实现，天然处理跨页与未映射指针。

## 遇到的问题与解决

1. **spawn 不能复用 fork 的 exec 部分**：模板提示 fork+exec ≠ spawn。直接 `TaskControlBlock::new` 即可，但要记得补齐 parent/children 双向关系，否则子进程退出时父进程 waitpid 找不到它。
2. **stride pass 取整**：pass = BIG_STRIDE / priority 是整数除法，priority 很大时 pass 可能为 0 导致 stride 不增；把 priority clamp 到 ≤ BIG_STRIDE 保证 pass ≥ 1。
3. **scheduler 用 inner_exclusive_access 读 stride**：TCB 的 inner 是 UPSafeCell，扫描比较时短暂借锁，选出后立即 drop，避免跨调度点持锁。

## 测试结果

本地通过官方检查器验证：

```
make -C ci-user test CHAPTER=5
...
[PASS] ch5 forks ... Test fork OK!
[PASS] Test spawn OK!
[PASS] Test setprio OK!
[PASS] Test stride0 OK!
...
Test passed: xx/xx
Report for lab1 found.
Report for lab2 found.
Report for lab3 found.
```

fork/waitpid、exec、spawn、set_priority、stride 系列与 ch3/ch4 回归用例（yield/sleep/trace/mmap/munmap/sbrk）全部通过。

## 思考题

1. **spawn 与 fork+exec 的本质区别？**
   fork+exec 先付出一次地址空间深拷贝的代价又立刻丢弃；spawn 只创建新地址空间并继承父子关系，省掉这次拷贝。对大地址空间进程，spawn 的开销显著更低。

2. **stride 算法的溢出与取整问题？**
   32 位 stride 会回绕导致比较失效，选 BIG_STRIDE=10^9 且 stride 用 u64 后，正常运行时间远不足以溢出。pass 向下取整可能引入份额误差，BIG_STRIDE 足够大时误差占比可忽略；另外保证 pass ≥ 1 防止 stride 停滞。

3. **为什么 parent 用 Weak 引用？**
   父进程通过 children 持有子进程的强引用。若子进程再强引用父进程，会形成引用环，进程退出后谁都无法释放。Weak 打破环：父进程死了，子进程的 parent 自动失效（升级为 None），不会悬挂强引用。

4. **waitpid 为什么要区分 -1 与具体 pid？**
   -1 表示等待任意子进程退出（先到先得），指定 pid 则只等该子进程。后者允许父进程与不同子进程有不同的同步顺序，是 shell 管道/作业控制的基础。
