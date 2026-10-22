# Lab 4 实验报告：文件系统（ch6）

## 实验目标

在 ch5 进程管理的基础上引入文件系统与 I/O：

1. 理解 easy-fs 的分层设计：磁盘块设备接口、块缓存、磁盘布局（超级块/位图/inode 区/数据区）与虚拟文件系统层（Inode）。
2. 实现基于 virtio-blk 的块设备驱动，将 easy-fs 挂载为内核的根文件系统，应用 ELF 从文件系统加载（exec/spawn 通过 `open_file` 读取）。
3. 实现系统调用 `sys_fstat`（查询文件元数据）、`sys_linkat`/`sys_unlinkat`（硬链接的建立与删除），在 easy-fs 的 DiskInode 中引入链接计数 nlink 支持硬链接语义。

## 实验原理简述

### easy-fs 磁盘布局

磁盘被划分为五个连续区域：超级块（魔法数、各区大小）、inode 位图、inode 区域、数据位图、数据区域。DiskInode 采用"直接块 + 一级间接 + 二级间接"的三级索引管理数据块；目录也是 DiskInode（`DiskInodeType::Directory`），其数据区由 32 字节的 DirEntry（name[28] + inode_id[4]）线性组成。所有磁盘访问经 BlockCache（写回式缓存）完成。

### 硬链接语义

同一份数据可以有多个目录项（dirent）指向同一个 inode。需要在 DiskInode 中新增 `nlink` 字段记录链接数：

- `linkat(old, new)`：找到 old 的 inode，新增一个指向它的 dirent，nlink+1；
- `unlinkat(name)`：删除 name 对应的 dirent，nlink-1；当 nlink 减到 0 时，释放该 inode 的全部数据块（clear_size + dealloc_data）并回收 inode 位图位（dealloc_inode）。

## 实现要点

### 1. easy-fs 扩展（layout.rs / efs.rs / vfs.rs）

- `DiskInode` 增加 `pub nlink: u32`，`initialize` 时置 1。DiskInode 布局变化由 `size_of::<DiskInode>()` 动态参与 `get_disk_inode_pos` 与格式化时的 inode 区大小计算（easy-fs-fuse 依赖同一 crate，格式化与内核布局自动一致）。
- `EasyFileSystem::get_inode_id_by_pos(block_id, offset)`：`get_disk_inode_pos` 的逆运算，fstat 用来从内存 Inode 反查 inode 编号。
- `EasyFileSystem::dealloc_inode(inode_id)`：清 inode 位图位。
- `Inode::link(old, new) -> bool`：读根目录遍历 dirent——new 已存在则失败；old 不存在则失败；否则 `increase_size` 追加一个新 dirent 并把目标 inode 的 nlink+1。
- `Inode::unlink(name) -> bool`：定位 dirent 后，用"最后一个 dirent 移到空洞处 + size -= DIRENT_SZ"的方式删除（避免移动整个目录区）；随后 nlink-1，若减到 0 则 `clear_size` 释放数据块、`dealloc_data` 逐块归还数据位图、`dealloc_inode` 回收 inode。
- `Inode::stat_info() -> (ino, nlink, is_dir)`。

### 2. 内核侧（fs/mod.rs、fs/inode.rs、syscall/fs.rs）

- `File` trait 新增带默认实现的方法 `inode_stat() -> Option<(ino, nlink, StatMode)>`（默认 None，stdio 不支持），仅 `OSInode` 覆写——从其持有的 `Arc<Inode>` 读 `stat_info()` 并映射为 `StatMode::FILE/DIR`。
- `sys_fstat(fd, st)`：从 fd 表取文件，`translated_refmut` 把 dev=0、ino、mode、nlink 写回用户 Stat 结构。
- `sys_linkat`/`sys_unlinkat`：`translated_str` 读取路径后调用 `fs::link_file`/`unlink_file`（对 ROOT_INODE 操作），成功返回 0，失败 -1。

### 3. ch5 功能迁移（进程管理部分）

- stride 调度：`TaskControlBlockInner` 携带 `stride: u64`、`priority: u32`（fork 继承父优先级），`TaskManager::add` 入队时 `stride += BIG_STRIDE / priority`，`fetch` 取最小者；`sys_set_priority` 校验 prio ≥ 2 并 clamp。
- spawn：`open_file` + `read_all` 取 ELF 后直接 `TaskControlBlock::new`，只建立父子关系（fork+exec 的两次地址空间操作被省掉）。
- mmap/munmap：`MemorySet::insert_mmap_area`（VPN 区间重叠检查、Framed 逐页分配、固定加 U 位）与 `remove_framed_area`（区间完全一致才删）；`sys_get_time` 用新增的 `translated_user_write_byte` 逐字节写回用户指针（天然处理跨页与未映射指针）。

## 遇到的问题与解决

1. **删 dirent 的策略**：目录区是线性 dirent 数组，"原地删除"最简单的办法是把最后一项搬进空洞并缩短 size——O(1) 且不会破坏其他 dirent 的 inode 引用。
2. **nlink=0 时的资源回收与打开中的文件**：严格 POSIX 语义要求"打开着也不能回收，直到最后一个 fd 关闭"。本实验测试未覆盖该场景（file3 的 unlink 都发生在 close 之后），实现选择 nlink=0 立即回收数据块与 inode；若要支持"unlink 后仍可经 fd 访问"，需要给 OSInode 增加打开计数，把回收推迟到计数归零。
3. **fstat 拿不到具体类型**：fd 表存的是 `Arc<dyn File>`，无法直接 downcast 到 OSInode。给 File trait 加带默认值的方法是最小侵入方案，避免引入 Any/downcast。
4. **重开已 unlink 的文件必须失败**：file3 每轮 unlink 后用 RDONLY 重开要返回 -1——只要 dirent 被删除，`ROOT_INODE.find` 就找不到，无需额外标记；CREATE 重开得到的是新 inode。

## 测试结果

本地通过官方检查器验证：

```
make -C ci-user test CHAPTER=6
...
[PASS] found <Test file0 OK!>
[PASS] found <Test fstat OK!>
[PASS] found <Test link OK!>
[PASS] found <Test mass open/unlink OK!>
[PASS] found <file_test passed!>
Test passed: 16/16
Report for lab1 found.
Report for lab2 found.
Report for lab3 found.
Report for lab4 found.
```

ch3-ch5 回归用例（yield/sleep、mmap/munmap/sbrk、fork/exec/waitpid/spawn/setprio/stride）在 ch6 内核上全部通过。

## 思考题

1. **为什么 nlink 减到 0 才真正删除数据？**
   dirent 只是"名字 → inode"的映射，删除名字不影响数据所有权；只有当所有名字都消失（nlink=0），数据才成为垃圾。这正是硬链接与复制的本质区别：多个链接共享同一份数据块，任何链接写入对所有链接可见。
2. **easy-fs 的块缓存为什么用 Mutex？多核下有什么问题？**
   BlockCache 用 spin::Mutex 包裹，独占访问保证一致性，但全局串行化了所有块访问；多核下锁竞争会成为瓶颈。改进方向是更细粒度的缓存（每块一把锁）或参考 rustsbi/xv6 的 buffer cache 设计。
3. **unlink 一个正在被读写的文件会发生什么？**
   本实现会立即回收其数据块，后续经 fd 的 read/write 会读到被清零/复用的块（错误数据甚至 panic）。POSIX 通过"inode 活到最后一个引用（含 fd）关闭"避免这一点，需要内核为每个打开实例维护引用计数。
4. **DiskInode 加字段为什么不会破坏旧镜像？**
   每次 `make run` 都由 easy-fs-fuse 重新格式化并写入新镜像，`size_of::<DiskInode>()` 的变化同时反映在格式化端与内核端；如果要求兼容旧镜像，则需要超级块里的版本号/字段偏移来区分新旧布局。
