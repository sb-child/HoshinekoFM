//! CLI 定义与解析。
//!
//! 支持三种入口模式：
//!
//! ```text
//! hnfm                              → 默认 Launch，复用或变 primary
//! hnfm /path1 /path2                → 带初始路径的 Launch
//! hnfm launch [options] [paths..]   → 显式 Launch
//! hnfm fs-worker --worker-id <n> --fd <n>  → Worker 子进程（内部）
//! ```

use clap::{Parser, Subcommand};

/// Hoshineko File Manager
#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    /// 子命令；省略时等同 `launch`
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// 要打开的初始路径 (快捷形式: `hnfm /path1 /path2`)
    #[arg(default_values_t = Vec::<String>::new())]
    pub paths: Vec<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 启动主应用（新建或挂载实例）
    Launch(LaunchCmd),

    /// 启动文件系统 Worker 子进程 (内部使用)
    FsWorker(FsWorkerCmd),
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

/// 启动主应用 / 挂载到现有实例。
#[derive(Parser)]
pub struct LaunchCmd {
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

// ---------------------------------------------------------------------------
// FsWorker
// ---------------------------------------------------------------------------

/// 启动文件系统 Worker 子进程。
///
/// 由主进程通过 `pkexec ...` 启动。
#[derive(Parser)]
pub struct FsWorkerCmd {
    /// Worker ID
    #[arg(long)]
    pub worker_id: u64,

    /// 请求通道 fd（app→worker，继承自父进程）
    #[arg(long)]
    pub fd: Option<i32>,

    /// 回调通道 fd（worker→app，继承自父进程）
    #[arg(long)]
    pub cb_fd: Option<i32>,
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

/// 解析结果：决定进程接下来的行为。
pub enum Dispatch {
    /// 启动/挂载 Launch 模式
    Launch(LaunchCmd),
    /// 启动 FS Worker 子进程
    FsWorker(FsWorkerCmd),
}

/// 解析命令行参数并返回执行分发结果。
pub fn parse() -> Dispatch {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Launch(mut cmd)) => {
            // 合并顶层 paths 和 subcommand paths
            if !cli.paths.is_empty() && cmd.paths.is_empty() {
                cmd.paths = cli.paths;
            }
            Dispatch::Launch(cmd)
        }
        Some(Commands::FsWorker(cmd)) => Dispatch::FsWorker(cmd),
        None => {
            // 无子命令 → 默认 Launch
            Dispatch::Launch(LaunchCmd {
                new_instance: false,
                instance_id: None,
                paths: cli.paths,
            })
        }
    }
}
