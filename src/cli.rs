use std::path::PathBuf;

#[derive(clap::Parser)]
#[command(version)]
pub struct Args {
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long)]
    pub log_dir: Option<PathBuf>,
    #[arg(long)]
    pub verbose: bool,
}
