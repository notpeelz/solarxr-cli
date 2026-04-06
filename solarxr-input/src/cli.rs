use std::path::PathBuf;

use clap::ValueHint;
use solarxr_common::VERSION;

#[derive(clap::Parser)]
#[command(version = VERSION, about)]
pub struct Args {
    #[arg(
        short = 's',
        long = "socket",
        help = "Use a different socket instead of the default",
        value_name = "FILE",
        value_hint = ValueHint::FilePath
    )]
    pub socket_path: Option<PathBuf>,
    #[arg(
        short = 'c',
        long = "config",
        help = "Use a different config file instead of the default",
        value_name = "FILE",
        value_hint = ValueHint::FilePath
    )]
    pub config_path: Option<PathBuf>,
}
