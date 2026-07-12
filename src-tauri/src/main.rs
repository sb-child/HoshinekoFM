// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    tracing::info!("hnfm starting");

    let dispatch = hnfm_lib::cli::parse();

    match dispatch {
        hnfm_lib::cli::Dispatch::Launch(args) => {
            let instance_id = args.instance_id.unwrap_or(std::process::id() as u64);

            // 尝试复用已有实例（--new-instance 跳过）
            if !args.new_instance {
                if hnfm_lib::appreuse::run_app_reuse(args.instance_id, &args.paths).await {
                    std::process::exit(0);
                }
            }

            // 启动新实例
            tracing::info!("launching new instance {instance_id}");
            hnfm_lib::app::run_app(hnfm_lib::app::RunOpts {
                instance_id,
                paths: args.paths,
            })
            .await;
        }

        hnfm_lib::cli::Dispatch::FsWorker(cmd) => {
            tracing::info!("launching fs-worker mode");
            let opts = hnfm_lib::fsworker::FsWorkerOpts {
                fs_worker_id: cmd.fs_worker_id,
                fd: cmd.fd,
                cb_fd: cmd.cb_fd,
                parent_pid: cmd.parent_pid,
            };
            hnfm_lib::fsworker::run_fs_worker(opts).await;
        }
    }
}
