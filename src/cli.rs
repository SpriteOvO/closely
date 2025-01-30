use std::path::PathBuf;

use shadow_rs::{formatcp, shadow};

shadow!(build);
use build::*;

const VER: &str = formatcp!(
    "{}, {BRANCH} ({SHORT_COMMIT}{}), {BUILD_TIME}, {BUILD_RUST_CHANNEL}",
    env!("CARGO_PKG_VERSION"),
    if !build::GIT_CLEAN { ", dirty" } else { "" }
);

#[derive(clap::Parser, Debug)]
#[command(version, long_version = VER)]
pub struct Args {
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long)]
    pub log_dir: Option<PathBuf>,
    #[arg(long)]
    pub verbose: bool,
}
