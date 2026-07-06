// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    tracing::info!("hnfm starting");

    let dispatch = hnfm_lib::cli::parse();

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    match dispatch {
        hnfm_lib::cli::Dispatch::Launch(cmd) => {
            // 1. 如果指定了 --instance-id，直接连接该实例
            if cmd.instance_id.is_some() {
                rt.block_on(hnfm_lib::appreuse::run_app_reuse(
                    cmd.instance_id,
                    &cmd.paths,
                ));
                // run_app_reuse 会 exit，不会走到这里
            }

            // 2. 尝试抢 primary（无论是否 --new-instance）
            let (be_primary, instance_id) = hnfm_lib::appreuse::try_acquire_primary();

            if be_primary {
                // 3a. 自己是 primary，启动 Tauri
                tracing::info!("launching as primary instance {instance_id}");
                hnfm_lib::app::run_app(hnfm_lib::app::RunOpts {
                    is_primary: true,
                    instance_id,
                    paths: cmd.paths,
                });
            } else if !cmd.new_instance {
                // 3b. 不是 primary 且未指定 --new-instance → 复用已有 primary（打开新窗口）
                tracing::info!(
                    "primary already exists, connecting to send open window request"
                );
                rt.block_on(hnfm_lib::appreuse::run_app_reuse(
                    None, // 连接到 primary
                    &cmd.paths,
                ));
                // run_app_reuse 会 exit
            } else {
                // 3c. --new-instance 但抢 primary 失败 → 仍启动独立实例
                tracing::info!("launching as secondary instance {instance_id}");
                hnfm_lib::app::run_app(hnfm_lib::app::RunOpts {
                    is_primary: false,
                    instance_id,
                    paths: cmd.paths,
                });
            }
        }

        hnfm_lib::cli::Dispatch::FsWorker(cmd) => {
            // 启动 FS Worker 子进程
            tracing::info!("launching fs-worker mode");
            let opts = hnfm_lib::fsworker::FsWorkerOpts {
                worker_id: cmd.worker_id,
                fd: cmd.fd,
            };
            rt.block_on(hnfm_lib::fsworker::run_fs_worker(opts));
        }
    }
}
