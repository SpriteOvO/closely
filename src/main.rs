mod cli;

use std::{fs, path::Path, process::exit, sync::Arc};

use anyhow::anyhow;
use clap::Parser;
use closely::prop;
use spdlog::{
    prelude::*,
    sink::{RotatingFileSink, RotationPolicy},
};

#[tokio::main]
async fn main() {
    let args = cli::Args::parse();
    let setup_logger_result = setup_logger(args.verbose, args.log_dir.as_deref());

    info!("{} startup!", prop::PACKAGE.name);
    info!("current version: {}", prop::PACKAGE.version);

    match (setup_logger_result, &args.log_dir) {
        (Ok(_), Some(log_dir)) => info!("logs will be written to '{}'", log_dir.display()),
        (Ok(_), None) => {
            warn!("logs will not be written to files, specify option '--log-dir' to enable it")
        }
        (Err(err), _) => {
            error!("logs will not be written to files, failed to setup logger: {err}")
        }
    }

    if let Err(err) = run(args).await {
        error!("exit with error: {err}");
        exit(1);
    }

    info!("exit normally");
}

fn setup_logger(verbose: bool, log_dir: Option<&Path>) -> anyhow::Result<()> {
    if verbose {
        spdlog::default_logger().set_level_filter(LevelFilter::All)
    }

    if let Some(log_dir) = log_dir {
        fs::create_dir_all(log_dir)
            .map_err(|err| anyhow!("failed to create log directory: {err}"))?;

        let file_sink = Arc::new(
            RotatingFileSink::builder()
                .base_path(log_dir.join("log.txt"))
                .rotation_policy(RotationPolicy::Daily { hour: 0, minute: 0 })
                .build()
                .map_err(|err| anyhow!("failed to build log file sink: {err}"))?,
        );

        let logger = spdlog::default_logger()
            .fork_with(|logger| {
                logger.sinks_mut().push(file_sink);
                Ok(())
            })
            .expect("failed to build logger");

        spdlog::set_default_logger(logger);
    }

    spdlog::default_logger().set_flush_level_filter(LevelFilter::All);

    Ok(())
}

async fn run(args: cli::Args) -> anyhow::Result<()> {
    closely::run(&args.config).await
}
