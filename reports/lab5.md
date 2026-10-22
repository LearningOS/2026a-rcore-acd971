# Lab 5 实验报告：并发（ch8）

## 实验目标

在前四章（地址空间、进程管理、文件系统）的基础上实现并发：

1. 理解线程（TaskControlBlock 即线程控制块）与进程的关系：同一进程内的多个线程共享地址空间与资源，各自拥有内核栈、TrapContext 与 tid。
2. 基于模板给定的同步原语（自旋/阻塞互斥锁、信号量、条件变量），补全其系统调用层的正确挂接，使 ch8b 的线程、 philosophers、race adder、mpsc、sync、condvar 用例全部通过。
3. 实现系统调用 `sys_enable_deadlock_detect` 与基于银行家算法的死锁检测：跟踪各进程对互斥锁/信号量的请求-持有关系，在不安全状态出现时报告死锁。

## 实验原理简述

### 线程模型

ch8 中 `TaskControlBlock` 直接表示线程：持有 `res`（tid、用户栈、TrapContext 物理页）、`task_cx`、内核栈；进程 `ProcessControlBlock` 持有 `memory_set`、fd 表、信号量/互斥锁/条件变量列表与线程数组。线程切换仍用 `__switch`，trap 保存/恢复在各自内核栈上。`sys_thread_create` 在进程内创建新线程（分配 tid/用户栈/trap_cx，伪造首次运行上下文）。

### 同步原语

- **MutexSpin**：原子指令忙等，适合临界区极短的场景。
- **MutexBlocking**：临界区被占时把当前线程置 Blocked 并挂入等待队列，调度其他线程，释放时唤醒队首。减少无效自旋开销。
- **Semaphore**：计数器 + 等待队列，`down` 计数为 0 时阻塞，`up` 计数加一并唤醒。
- **Condvar**：与互斥锁配合的等待/唤醒队列，`wait(mutex)` 原子地"解锁 + 睡眠 + 重锁"。

### 死锁检测（银行家算法）

对每个进程维护独立账本：各线程的 `allocation[tid][rid]`（已持有）与 `need[tid][rid]`（尚需，含阻塞中的请求），进程级 `available[rid]`（剩余）。互斥锁是计数为 1 的资源，信号量是计数为 N 的资源；两者统一编号（信号量 id 加偏移 2^16）。线程请求资源时先"试探性"记账并做安全序列检查：若存在一个调度序列使所有线程都能拿到所需资源并完成，则状态安全、允许阻塞等待；否则立即向调用线程返回 -0xdead 报告死锁，而不是让它永久阻塞。检测默认关闭，仅对调用过 `sys_enable_deadlock_detect(1)` 的进程生效。

## 实现要点

### 1. 同步系统调用挂接（模板原语已给，补资源层）

- `sys_mutex_create` / `sys_semaphore_create`：创建原语的同时调用 `deadlock_detect::add_resource_pool(pid, rid, count)` 注册资源池（互斥锁 1 单位，信号量 res_count 单位）。
- `sys_mutex_lock`：若检测开启，先 `request_resource`，不安全则返回 -0xdead；否则锁成功后 `acquire_resource` 记账。
- `sys_mutex_unlock`：解锁后 `release_resource` 归还。
- `sys_semaphore_down/up`：同上，资源 id 为 `SEM_BASE + sem_id`。
- 条件变量不参与检测（其内部等待依赖互斥锁）。

### 2. sys_enable_deadlock_detect

按进程粒度开关：`set_enable(pid, enabled != 0)`，返回 0。检测状态与资源账本集中在一个 `UPSafeCell<BTreeMap<pid, ProcState>>` 中，每个启用检测的进程有独立的 available/allocation/need 账本（并发运行的应用即使复用相同 rid 也互不干扰），键为 BTreeMap 稀疏存储，避免为未使用检测的进程分配内存。

### 3. 检测器核心（sync/deadlock_detect.rs）

账本按"进程 → 线程"两级组织（`ThreadRes` 持有各线程的 allocation/need）：

- `request(pid, tid, rid)`：`need[tid][rid] += 1` 后跑安全检查，失败则回滚记账并返回 false，内核据此向调用线程返回 -0xdead（不阻塞、不 panic）；
- `is_safe()`：经典银行家循环——在 `work = available` 副本上反复寻找"所有需求都可满足"的线程，模拟其完成并释放持有量；一轮无进展即终止，所有线程都能完成则安全。**阻塞中的请求保留在 need 里**，因此环形等待（A 等 B 持有的资源、B 又等 A）会在最后一个 request 到达时被识别为不安全；
- `acquire/release`：维护 allocation 与 available 增减；release 对无对应 acquire 的调用静默忽略（如线程收到死锁报告退出后的补偿 up）。

### 4. 前四章系统调用移植

- `sys_fstat/linkat/unlinkat` + easy-fs nlink 硬链接（与 ch6 实现一致）；
- `sys_get_time`（逐字节页表翻译写回 TimeVal，处理跨页）、`sys_mmap/munmap`（MemorySet 区间重叠检查/精确匹配删除，固定附加 U 位）；
- `sys_spawn`：`open_file` 读 ELF 后 `ProcessControlBlock::new` 直接建进程（省掉 fork 的地址空间深拷贝），补父子关系；
- `sys_set_priority`：ch8 无 stride 调度，仅做 `prio >= 2` 的参数校验；
- easy-fs 块缓存扩到 4096 块，`dealloc_data` 不再逐字节清零（释放块是垃圾数据，重分配路径必然整块覆写），大幅降低大量创建/删除文件的 I/O 量。

## 遇到的问题与解决

1. **检测粒度选择**：账本必须区分到线程——若把进程内所有线程合并记账，sem2 用例中"合法的 BARRIER 等待"会被误判为死锁；按线程维护 allocation/need 后，banker 模拟才能正确推断"持有资源的线程终将完成并释放"。仅对启用检测的进程记账，其余进程零开销。
2. **available 的初始化**：资源池在 create 时才注册，避免 acquire 时对不存在的计数做下溢减法；互斥锁固定 1 单位、信号量按初始计数注册。
3. **spin 锁是否参与检测**：spin 锁不会睡眠，不产生等待边；但为了账本一致性（enable 后同一锁的 lock/unlock 必须配对记账），统一参与。未启用检测的进程完全跳过，race adder 等 spin 用例不受影响。
4. **检测到死锁后的行为**：向调用线程返回 -0xdead（-57005）而非 panic——应用据此知道请求会死锁并自行退出，内核与其他进程继续运行；测试框架按应用打印的 OK 行判定通过。

## 测试结果

本地通过官方检查器验证：

```
make -C ci-user test CHAPTER=8
...
Test passed: 25/25
Report for lab1 found.
...
Report for lab5 found.
```

ch2-ch7 回归用例（yield、fork、file、pipe）、ch8b 全部并发用例（threads/mpsc/philosophers/race adder/condvar）与三个 deadlock 用例全部通过。

## 思考题

1. **银行家算法为什么"安全"但保守？**
   安全检查只看"存在一种让所有进程完成的调度"，不代表实际会这样调度；判定不安全（可能死锁）就拒绝分配，会牺牲并行度——进程本可以等一等拿到资源。正确性与吞吐之间的折衷。
2. **MutexSpin 与 MutexBlocking 各适用什么场景？**
   Spin 适合多核 + 临界区极短：切换成本 > 忙等成本；单核上 spin 是纯浪费（持有者根本没机会运行）。Blocking 适合临界区较长或单核场景，代价是两次线程切换。
3. **条件变量为什么要与互斥锁绑定？**
   "检查条件 + 挂起"必须原子，否则在 check 与 sleep 之间另一线程修改条件并 signal，唤醒信号就会丢失（lost wakeup）。wait(mutex) 在内核里原子完成解锁-入队-睡眠，醒来重拿锁。
4. **为什么 unlink 之后 nlink=0 还要考虑打开计数？**
   若文件在打开状态下被 unlink，数据块立即回收会让已打开的 fd 指向已释放的存储。POSIX 通过 inode 活到"最后一个引用（目录项 + fd）关闭"解决；本实现按 nlink 立即回收，依赖测试中先 close 再 unlink 的使用顺序。
