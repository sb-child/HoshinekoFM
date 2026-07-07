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
            if cmd.instance_id.is_some() {
                let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
                rt.block_on(hnfm_lib::appreuse::run_app_reuse(
                    cmd.instance_id,
                    &cmd.paths,
                ));
            }

            let (be_primary, instance_id) = hnfm_lib::appreuse::try_acquire_primary();

            if be_primary {
                tracing::info!("launching as primary instance {instance_id}");
                hnfm_lib::app::run_app(hnfm_lib::app::RunOpts {
                    is_primary: true,
                    instance_id,
                    paths: cmd.paths,
                });
            } else if !cmd.new_instance {
                tracing::info!("primary already exists, connecting to send open window request");
                let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
                rt.block_on(hnfm_lib::appreuse::run_app_reuse(None, &cmd.paths));
            } else {
                tracing::info!("launching as secondary instance {instance_id}");
                hnfm_lib::app::run_app(hnfm_lib::app::RunOpts {
                    is_primary: false,
                    instance_id,
                    paths: cmd.paths,
                });
            }
        }

        hnfm_lib::cli::Dispatch::FsWorker(cmd) => {
            tracing::info!("launching fs-worker mode");
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
            let opts = hnfm_lib::fsworker::FsWorkerOpts {
                worker_id: cmd.worker_id,
                fd: cmd.fd,
            };
            rt.block_on(hnfm_lib::fsworker::run_fs_worker(opts));
        }
    }
}
