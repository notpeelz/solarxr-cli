use std::{env, io, path::PathBuf, process::ExitCode};

use eyre::{Result, eyre};
use serde_json::json;
use solarxr_client::{BodyPart, SolarXRClient};
use tokio::net::UnixStream;
use tracing_subscriber::EnvFilter;

use crate::cli::{Command, ResetCommand, StayAlignedCommand, TrackingCommand};

mod cli;

impl From<cli::StayAlignedPose> for solarxr_client::StayAlignedPose {
    fn from(value: cli::StayAlignedPose) -> Self {
        match value {
            cli::StayAlignedPose::Standing => solarxr_client::StayAlignedPose::Standing,
            cli::StayAlignedPose::Sitting => solarxr_client::StayAlignedPose::Sitting,
            cli::StayAlignedPose::Flat => solarxr_client::StayAlignedPose::Flat,
        }
    }
}

async fn exec() -> Result<ExitCode> {
    let args = <crate::cli::Args as clap::Parser>::parse();

    let socket_paths = {
        let mut socket_paths = Vec::<PathBuf>::new();
        if let Some(socket_path) = args.socket_path {
            socket_paths.push(socket_path);
        } else {
            let xdg_runtime_dir = PathBuf::from(
                env::var("XDG_RUNTIME_DIR")
                    .map_err(|_| eyre!("env var XDG_RUNTIME_DIR not set"))?,
            );
            socket_paths.push(xdg_runtime_dir.join("SlimeVRRpc"));
            socket_paths.push(xdg_runtime_dir.join("app/dev.slimevr.SlimeVR/SlimeVRRpc"));
        }
        socket_paths
    };

    let connect = async || -> Result<SolarXRClient> {
        assert!(!socket_paths.is_empty());
        let mut socket_paths = socket_paths.iter();
        let mut last_err = Option::<eyre::Error>::None;
        let stream = 'f: loop {
            let Some(socket_path) = socket_paths.next() else {
                break 'f Err(last_err.unwrap());
            };
            match UnixStream::connect(socket_path).await {
                Ok(stream) => break 'f Ok(stream),
                Err(err) => {
                    if err.kind() == io::ErrorKind::NotFound {
                        last_err = Some(eyre!("socket file doesn't exist; is the server running?"));
                    } else {
                        last_err = Some(err.into());
                    }
                }
            };
        }?;
        let client = SolarXRClient::new(stream);
        Ok(client)
    };

    match args.command {
        Command::Tracking {
            command: TrackingCommand::Pause,
        } => {
            let mut client = connect().await?;
            client.set_pause_tracking(true).await?;
        }
        Command::Tracking {
            command: TrackingCommand::Unpause,
        } => {
            let mut client = connect().await?;
            client.set_pause_tracking(false).await?;
        }
        Command::Tracking {
            command: TrackingCommand::Toggle,
        } => {
            let mut client = connect().await?;
            let paused = client.pause_tracking_state().await?;
            client.set_pause_tracking(!paused).await?;
        }
        Command::Tracking {
            command:
                TrackingCommand::IsPaused {
                    exit_code_unpaused,
                    exit_code_paused,
                    quiet,
                },
        } => {
            let mut client = connect().await?;
            let paused = client.pause_tracking_state().await?;
            if paused {
                if !quiet {
                    println!("paused");
                }
                if let Some(exit_code) = exit_code_paused {
                    return Ok(exit_code.into());
                }
            } else {
                if !quiet {
                    println!("unpaused");
                }
                if let Some(exit_code) = exit_code_unpaused {
                    return Ok(exit_code.into());
                }
            }
        }
        Command::Reset {
            command: ResetCommand::Yaw { delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client.reset_yaw().await?;
        }
        Command::Reset {
            command: ResetCommand::Mounting { delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client.reset_mounting().await?;
        }
        Command::Reset {
            command: ResetCommand::MountingFeet { delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client
                .reset_mounting_with_parts(&[BodyPart::LEFT_FOOT, BodyPart::RIGHT_FOOT])
                .await?;
        }
        Command::Reset {
            command: ResetCommand::MountingFingers { delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client
                .reset_mounting_with_parts(&[
                    BodyPart::LEFT_THUMB_METACARPAL,
                    BodyPart::LEFT_THUMB_PROXIMAL,
                    BodyPart::LEFT_THUMB_DISTAL,
                    BodyPart::LEFT_INDEX_PROXIMAL,
                    BodyPart::LEFT_INDEX_INTERMEDIATE,
                    BodyPart::LEFT_INDEX_DISTAL,
                    BodyPart::LEFT_MIDDLE_PROXIMAL,
                    BodyPart::LEFT_MIDDLE_INTERMEDIATE,
                    BodyPart::LEFT_MIDDLE_DISTAL,
                    BodyPart::LEFT_RING_PROXIMAL,
                    BodyPart::LEFT_RING_INTERMEDIATE,
                    BodyPart::LEFT_RING_DISTAL,
                    BodyPart::LEFT_LITTLE_PROXIMAL,
                    BodyPart::LEFT_LITTLE_INTERMEDIATE,
                    BodyPart::LEFT_LITTLE_DISTAL,
                    BodyPart::RIGHT_THUMB_METACARPAL,
                    BodyPart::RIGHT_THUMB_PROXIMAL,
                    BodyPart::RIGHT_THUMB_DISTAL,
                    BodyPart::RIGHT_INDEX_PROXIMAL,
                    BodyPart::RIGHT_INDEX_INTERMEDIATE,
                    BodyPart::RIGHT_INDEX_DISTAL,
                    BodyPart::RIGHT_MIDDLE_PROXIMAL,
                    BodyPart::RIGHT_MIDDLE_INTERMEDIATE,
                    BodyPart::RIGHT_MIDDLE_DISTAL,
                    BodyPart::RIGHT_RING_PROXIMAL,
                    BodyPart::RIGHT_RING_INTERMEDIATE,
                    BodyPart::RIGHT_RING_DISTAL,
                    BodyPart::RIGHT_LITTLE_PROXIMAL,
                    BodyPart::RIGHT_LITTLE_INTERMEDIATE,
                    BodyPart::RIGHT_LITTLE_DISTAL,
                ])
                .await?;
        }
        Command::Reset {
            command: ResetCommand::Full { delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client.reset_full().await?;
        }
        Command::StayAligned {
            command: StayAlignedCommand::SavePose { pose, delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client.save_stay_aligned_pose(pose.into()).await?;
        }
        Command::StayAligned {
            command: StayAlignedCommand::ResetPose { pose },
        } => {
            let mut client = connect().await?;
            client.reset_stay_aligned_pose(pose.into()).await?;
        }
        Command::StayAligned {
            command: StayAlignedCommand::Enable,
        } => {
            let mut client = connect().await?;
            client.set_stay_aligned_enabled(true).await?;
        }
        Command::StayAligned {
            command: StayAlignedCommand::Disable,
        } => {
            let mut client = connect().await?;
            client.set_stay_aligned_enabled(false).await?;
        }
        Command::StayAligned {
            command: StayAlignedCommand::Toggle,
        } => {
            let mut client = connect().await?;
            let enabled = client.stay_aligned_enabled().await?;
            client.set_stay_aligned_enabled(!enabled).await?;
        }
        Command::Info {
            command: cli::InfoCommand::Height { json },
        } => {
            let mut client = connect().await?;
            let height = client.get_height().await?;
            if json {
                println!(
                    "{}",
                    json!({
                        "min": height.min,
                        "max": height.max,
                    })
                );
            } else {
                println!("min: {}", height.min);
                println!("max: {}", height.max);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(debug_assertions)]
const DEFAULT_LOG_FILTER: &str = "solarxr_client=trace";

#[cfg(not(debug_assertions))]
const DEFAULT_LOG_FILTER: &str = "solarxr_client=warn";

fn main() -> ExitCode {
    let directives = env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LOG_FILTER.to_owned());
    let env_filter = EnvFilter::builder().parse_lossy(directives);
    tracing_subscriber::fmt()
        .compact()
        .without_time()
        .with_writer(io::stderr)
        .with_env_filter(env_filter)
        .init();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            match exec().await {
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
                Ok(exit_code) => exit_code,
            }
        })
}
