//! CLI 定义与解析。
//!
//! 支持两种入口模式：
//!
//! ```text
//! hnfm [options] [paths..]          → 默认 Launch，复用或变 primary
//! hnfm __fs-worker --fs-worker-id <n> --fd <n> --cb-fd <n> --parent-pid <n>
//!                                   → FS Worker 子进程（内部）
//! ```
//!
//! 设计要点：不提供 `launch` 子命令，避免 `hnfm launch` 与目录名冲突。
//! `--new-instance` / `--instance-id` 直接作为顶层 flag。

use clap::{Parser, Subcommand};

/// Hoshineko File Manager
#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    /// 子命令；省略时等同 launch
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// 强制启动新实例（不尝试复用已有 primary）
    #[arg(long, default_value_t = false)]
    pub new_instance: bool,

    /// 挂载到指定实例 ID，或以此 ID 启动
    #[arg(long)]
    pub instance_id: Option<u64>,

    /// 要打开的初始路径
    #[arg(default_values_t = Vec::<String>::new())]
    pub paths: Vec<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 启动文件系统 Worker 子进程 (内部使用)
    #[command(name = "__fs-worker", hide = true)]
    FsWorker(FsWorkerCmd),
}

// ---------------------------------------------------------------------------
// Launch args
// ---------------------------------------------------------------------------

/// Launch 模式的启动参数。
pub struct LaunchArgs {
    pub new_instance: bool,
    pub instance_id: Option<u64>,
    pub paths: Vec<String>,
}

// ---------------------------------------------------------------------------
// FsWorker
// ---------------------------------------------------------------------------

/// 启动文件系统 Worker 子进程。
///
/// 由主进程通过 `pkexec ...` 启动。
#[derive(Parser)]
pub struct FsWorkerCmd {
    /// FS Worker ID
    #[arg(long = "fs-worker-id")]
    pub fs_worker_id: u64,

    /// 请求通道 fd（app→worker，继承自父进程）
    #[arg(long)]
    pub fd: i32,

    /// 回调通道 fd（worker→app，继承自父进程）
    #[arg(long)]
    pub cb_fd: i32,

    /// 主进程 PID（用于 Worker 侧孤儿检测）
    #[arg(long)]
    pub parent_pid: u32,
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

/// 解析结果：决定进程接下来的行为。
pub enum Dispatch {
    /// 启动/挂载 Launch 模式
    Launch(LaunchArgs),
    /// 启动 FS Worker 子进程
    FsWorker(FsWorkerCmd),
}

/// 解析命令行参数并返回执行分发结果。
pub fn parse() -> Dispatch {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::FsWorker(cmd)) => Dispatch::FsWorker(cmd),
        _ => Dispatch::Launch(LaunchArgs {
            new_instance: cli.new_instance,
            instance_id: cli.instance_id,
            paths: cli.paths,
        }),
    }
}
