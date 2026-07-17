# AGENTS.md — Hoshineko File Manager

正在大幅度重构: electron -> tauri

- 前端(vite, react, material web, lit): `src/`
- 后端(rust, tokio): `src-tauri/`

## 注意事项

每次更改都要写 devlog !!

设计准则：
这是文件管理器，跑在用户的桌面上。这不是容许3秒卡顿的网络服务集群。
应该假设所有文件，网络操作都是可能有延迟，可能出错的。即使是创建一个空文件。
程序不应该因为阻塞而卡顿，不应为了连接另一个实例/因为另一个实例断开连接而锁死或崩溃。
每个文件操作应该明确(什么操作，成功了，或者因为什么失败)，用户应该能明确下达命令和知道怎么回事。

### 前端

一定要写jsdoc！写字段说明！！！写i18n！！

先检查会不会破坏别的代码逻辑和功能。会的话跟我说！！

### 后端

添加依赖用 `cargo add` 而不是编辑 `Cargo.toml`。依赖版本应该用最新的。

最高的优先级是安全和性能！

写注释，写文档！

不要写太长的函数，不同的功能要拆分到不同的文件！

先检查会不会破坏别的代码逻辑和功能。会的话跟我说！！

**写完问自己这些问题（代码可读性和健壮性）**

1. 整洁度，文档完善度

- 代码本身足够描述代码的功能吗
- 人容易在有限的视野里看明白代码逻辑吗？（根据调用链优化代码布局，文件树等。注释说明这个类型/函数会在哪里被使用）
- 注释/文档清晰吗？每个函数和文件顶部都有注释和文档吗？有临时注释要清除吗？
- 是否存在`-----------`/`==========`/`↔ → —`等字符(去掉或者用键盘能打的字符替代)？
- 是否有未明确注释的不明意义行为(或者可以理清代码让它更清晰吗)
- 大缩进(>=4 tabs)能拆分吗？能展平吗？

2. 日志 / tracing

- 有多次出现的字段吗:
```rust
// 多余的 worker id
let worker_span = tracing::info_span!("worker", id = %cmd.fs_worker_id);
spawn(async { /* ... */ warn!("worker id {worker_id} crashed");}).instrument(worker_span);

// 改进后
let worker_span = tracing::info_span!("worker", id = %cmd.fs_worker_id);
spawn(async { /* ... */ warn!("crashed");}).instrument(worker_span);

// 多余的 service worker 和 worker_id
#[instrument(name = "service worker")]
async fn start_worker(worker_id: u64) { debug!("service worker: id={worker_id} starting"); /* ... */ }

// 改进后
#[instrument(name = "service worker")]
async fn start_worker(worker_id: u64) { debug!("starting"); /* ... */ }
```
- `tracing::Instrument` 应该注入到程序的所有地方(spawn, 函数块)。在关键区域注明 `tracing::info_span!()`
- `src-tauri/src/main.rs`的`worker_span`往下传递了吗

3. hack，workaround, hotfix

- 不必要的全局变量，不必要的硬编码，不必要的sleep
- 意义不明的 `bool` 参数 -> 语义清晰的 `enum`

4. 错误处理

- `Result<T, String>` → `https://docs.rs/snafu/latest/snafu/`
- 所有的spawn，处理过里面的错误吗，能把里面的错误传回handle吗
- `.unwrap()` safety? `unsafe { ... }` safety? 调用了可能`panic!()`的函数？如果spawn内部panic了怎么办？
- 不可恢复错误 `Err` 能传回程序入口点并优雅处理吗？如果在保持优雅的情况下不能，那可以`tracing::error!()`吗。

5. 异步代码和性能

- 不必要的copy？async里跑阻塞代码？spawn_blocking 里面如果有循环，能在需要cancel时打断吗？
- async里的await能一起执行而不是挨个await吗？sync会卡多久？如果需要，sync能cancel吗？
- mutex竞争？能换成更好的架构吗(channel)？
