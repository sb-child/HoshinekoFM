// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    tracing::info!("hnfm starting");

    let dispatch = hnfm_lib::cli::parse();

    match dispatch {
        hnfm_lib::cli::Dispatch::Launch(cmd) => {
            let instance_id = cmd.instance_id.unwrap_or(std::process::id() as u64);

            // 尝试复用已有实例（--new-instance 跳过）
            if !cmd.new_instance {
                let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
                if rt.block_on(hnfm_lib::appreuse::run_app_reuse(
                    cmd.instance_id,
                    &cmd.paths,
                )) {
                    std::process::exit(0);
                }
            }

            // 启动新实例
            tracing::info!("launching new instance {instance_id}");
            hnfm_lib::app::run_app(hnfm_lib::app::RunOpts {
                instance_id,
                paths: cmd.paths,
            });
        }

        hnfm_lib::cli::Dispatch::FsWorker(cmd) => {
            tracing::info!("launching fs-worker mode");
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
            let opts = hnfm_lib::fsworker::FsWorkerOpts {
                worker_id: cmd.worker_id,
                fd: cmd.fd,
                cb_fd: cmd.cb_fd,
            };
            rt.block_on(hnfm_lib::fsworker::run_fs_worker(opts));
        }
    }
}
