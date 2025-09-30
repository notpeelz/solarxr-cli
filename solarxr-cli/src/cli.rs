use std::{path::PathBuf, time::Duration};

use clap::{ValueHint, arg, crate_version};

// Cargo enforces semver, so have to resort to this hack to
// strip the major version component.
const fn format_version(s: &str) -> &str {
    assert!(s.is_ascii());
    let mut bytes = s.as_bytes();
    let mut dot_count = 0;
    loop {
        match bytes {
            [] => panic!("invalid version string"),
            [b'.', remaining @ ..] => {
                dot_count += 1;
                bytes = remaining;
            }
            [..] if dot_count == 1 => {
                break;
            }
            [_, remaining @ ..] => {
                bytes = remaining;
            }
        }
    }
    unsafe { &str::from_utf8_unchecked(bytes) }
}

const VERSION: &str = format_version(crate_version!());

#[derive(clap::Parser)]
#[command(version = VERSION, about)]
pub struct Args {
    #[arg(
        short = 's',
        long = "socket-path",
        help = "Use a different socket instead of the default",
        value_hint = ValueHint::FilePath
    )]
    pub socket_path: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

fn parse_duration_secs(arg: &str) -> Result<Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(Duration::from_secs(seconds))
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum StayAlignedPose {
    Standing,
    Sitting,
    Flat,
}

#[derive(clap::Subcommand)]
pub enum TrackingCommand {
    /// Pause tracking
    Pause,
    /// Unpause tracking
    #[command(alias = "resume")]
    Unpause,
    /// Toggle tracking
    Toggle,
    /// Query if tracking is paused
    IsPaused {
        #[arg(
            long = "set-exit-code-unpaused",
            help = "Set a custom exit code if unpaused",
            value_name = "VALUE"
        )]
        exit_code_unpaused: Option<u8>,
        #[arg(
            long = "set-exit-code-paused",
            help = "Set a custom exit code if paused",
            value_name = "VALUE"
        )]
        exit_code_paused: Option<u8>,
        #[arg(
            short = 'q',
            long = "quiet",
            help = "Don't echo the paused state to stdout"
        )]
        quiet: bool,
    },
}

#[derive(clap::Subcommand)]
pub enum ResetCommand {
    /// Request yaw reset
    Yaw {
        #[arg(
            short = 'd',
            long = "delay",
            help = "Delay (seconds) before sending the command",
            value_parser = parse_duration_secs,
        )]
        delay: Option<Duration>,
    },
    /// Request mounting reset
    Mounting {
        #[arg(
            short = 'd',
            long = "delay",
            help = "Delay (seconds) before sending the command",
            value_parser = parse_duration_secs,
        )]
        delay: Option<Duration>,
    },
    /// Request feet mounting reset
    MountingFeet {
        #[arg(
            short = 'd',
            long = "delay",
            help = "Delay (seconds) before sending the command",
            value_parser = parse_duration_secs,
        )]
        delay: Option<Duration>,
    },
    /// Request finger mounting reset
    MountingFingers {
        #[arg(
            short = 'd',
            long = "delay",
            help = "Delay (seconds) before sending the command",
            value_parser = parse_duration_secs,
        )]
        delay: Option<Duration>,
    },
    /// Request full reset
    Full {
        #[arg(
            short = 'd',
            long = "delay",
            help = "Delay (seconds) before sending the command",
            value_parser = parse_duration_secs,
        )]
        delay: Option<Duration>,
    },
}

#[derive(clap::Subcommand)]
pub enum StayAlignedCommand {
    /// Save "Stay Aligned" pose
    SavePose {
        #[arg(rename_all = "kebab-case")]
        pose: StayAlignedPose,
        #[arg(
            short = 'd',
            long = "delay",
            help = "Delay (seconds) before sending the command",
            value_parser = parse_duration_secs,
        )]
        delay: Option<Duration>,
    },
    /// Reset "Stay Aligned" pose
    ResetPose {
        #[arg(rename_all = "kebab-case")]
        pose: StayAlignedPose,
    },
    /// Enable "Stay Aligned"
    Enable,
    /// Disable "Stay Aligned"
    Disable,
    /// Toggle "Stay Aligned"
    Toggle,
}

#[derive(clap::Subcommand)]
pub enum InfoCommand {
    /// Query the minimum and maximum positional tracker height
    Height {
        #[arg(short = 'j', long = "json", help = "Format output as JSON")]
        json: bool,
    },
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Manage tracking state
    Tracking {
        #[command(subcommand)]
        command: TrackingCommand,
    },
    /// Perform tracker resets (yaw, full, mounting, etc.)
    Reset {
        #[command(subcommand)]
        command: ResetCommand,
    },
    /// Manage the configuration of "Stay Aligned "
    StayAligned {
        #[command(subcommand)]
        command: StayAlignedCommand,
    },
    /// Query miscellaneous information
    Info {
        #[command(subcommand)]
        command: InfoCommand,
    },
}
