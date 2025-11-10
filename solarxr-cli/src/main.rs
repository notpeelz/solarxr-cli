use std::{env, io, path::PathBuf, process::ExitCode};

use eyre::{Result, eyre};
use serde_json::json;
use solarxr_client::SolarXRClient;
use tokio::net::UnixStream;
use tracing_subscriber::EnvFilter;

mod cli;

use solarxr_client::proto;

impl From<cli::StayAlignedPose> for proto::rpc::StayAlignedRelaxedPose {
    fn from(value: cli::StayAlignedPose) -> Self {
        match value {
            cli::StayAlignedPose::Standing => proto::rpc::StayAlignedRelaxedPose::STANDING,
            cli::StayAlignedPose::Sitting => proto::rpc::StayAlignedRelaxedPose::SITTING,
            cli::StayAlignedPose::Flat => proto::rpc::StayAlignedRelaxedPose::FLAT,
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
        cli::Command::Tracking {
            command: cli::TrackingCommand::Pause,
        } => {
            let mut client = connect().await?;
            client.set_pause_tracking(true).await?;
        }
        cli::Command::Tracking {
            command: cli::TrackingCommand::Unpause,
        } => {
            let mut client = connect().await?;
            client.set_pause_tracking(false).await?;
        }
        cli::Command::Tracking {
            command: cli::TrackingCommand::Toggle,
        } => {
            let mut client = connect().await?;
            let paused = client.pause_tracking_state().await?;
            client.set_pause_tracking(!paused).await?;
        }
        cli::Command::Tracking {
            command:
                cli::TrackingCommand::IsPaused {
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
        cli::Command::Reset {
            command: cli::ResetCommand::Yaw { delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client.reset_yaw().await?;
        }
        cli::Command::Reset {
            command: cli::ResetCommand::Mounting { delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client.reset_mounting().await?;
        }
        cli::Command::Reset {
            command: cli::ResetCommand::MountingFeet { delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client
                .reset_mounting_with_parts(&[
                    proto::datatypes::BodyPart::LEFT_FOOT,
                    proto::datatypes::BodyPart::RIGHT_FOOT,
                ])
                .await?;
        }
        cli::Command::Reset {
            command: cli::ResetCommand::MountingFingers { delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client
                .reset_mounting_with_parts(&[
                    proto::datatypes::BodyPart::LEFT_THUMB_METACARPAL,
                    proto::datatypes::BodyPart::LEFT_THUMB_PROXIMAL,
                    proto::datatypes::BodyPart::LEFT_THUMB_DISTAL,
                    proto::datatypes::BodyPart::LEFT_INDEX_PROXIMAL,
                    proto::datatypes::BodyPart::LEFT_INDEX_INTERMEDIATE,
                    proto::datatypes::BodyPart::LEFT_INDEX_DISTAL,
                    proto::datatypes::BodyPart::LEFT_MIDDLE_PROXIMAL,
                    proto::datatypes::BodyPart::LEFT_MIDDLE_INTERMEDIATE,
                    proto::datatypes::BodyPart::LEFT_MIDDLE_DISTAL,
                    proto::datatypes::BodyPart::LEFT_RING_PROXIMAL,
                    proto::datatypes::BodyPart::LEFT_RING_INTERMEDIATE,
                    proto::datatypes::BodyPart::LEFT_RING_DISTAL,
                    proto::datatypes::BodyPart::LEFT_LITTLE_PROXIMAL,
                    proto::datatypes::BodyPart::LEFT_LITTLE_INTERMEDIATE,
                    proto::datatypes::BodyPart::LEFT_LITTLE_DISTAL,
                    proto::datatypes::BodyPart::RIGHT_THUMB_METACARPAL,
                    proto::datatypes::BodyPart::RIGHT_THUMB_PROXIMAL,
                    proto::datatypes::BodyPart::RIGHT_THUMB_DISTAL,
                    proto::datatypes::BodyPart::RIGHT_INDEX_PROXIMAL,
                    proto::datatypes::BodyPart::RIGHT_INDEX_INTERMEDIATE,
                    proto::datatypes::BodyPart::RIGHT_INDEX_DISTAL,
                    proto::datatypes::BodyPart::RIGHT_MIDDLE_PROXIMAL,
                    proto::datatypes::BodyPart::RIGHT_MIDDLE_INTERMEDIATE,
                    proto::datatypes::BodyPart::RIGHT_MIDDLE_DISTAL,
                    proto::datatypes::BodyPart::RIGHT_RING_PROXIMAL,
                    proto::datatypes::BodyPart::RIGHT_RING_INTERMEDIATE,
                    proto::datatypes::BodyPart::RIGHT_RING_DISTAL,
                    proto::datatypes::BodyPart::RIGHT_LITTLE_PROXIMAL,
                    proto::datatypes::BodyPart::RIGHT_LITTLE_INTERMEDIATE,
                    proto::datatypes::BodyPart::RIGHT_LITTLE_DISTAL,
                ])
                .await?;
        }
        cli::Command::Reset {
            command: cli::ResetCommand::Full { delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client.reset_full().await?;
        }
        cli::Command::StayAligned {
            command: cli::StayAlignedCommand::SavePose { pose, delay },
        } => {
            let mut client = connect().await?;
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            client.save_stay_aligned_pose(pose.into()).await?;
        }
        cli::Command::StayAligned {
            command: cli::StayAlignedCommand::ResetPose { pose },
        } => {
            let mut client = connect().await?;
            client.reset_stay_aligned_pose(pose.into()).await?;
        }
        cli::Command::StayAligned {
            command: cli::StayAlignedCommand::Enable,
        } => {
            let mut client = connect().await?;
            client.set_stay_aligned_enabled(true).await?;
        }
        cli::Command::StayAligned {
            command: cli::StayAlignedCommand::Disable,
        } => {
            let mut client = connect().await?;
            client.set_stay_aligned_enabled(false).await?;
        }
        cli::Command::StayAligned {
            command: cli::StayAlignedCommand::Toggle,
        } => {
            let mut client = connect().await?;
            let enabled = client.stay_aligned_enabled().await?;
            client.set_stay_aligned_enabled(!enabled).await?;
        }
        cli::Command::Info {
            command: cli::InfoCommand::Height { json },
        } => {
            let mut client = connect().await?;
            let msg = client.get_height().await?;
            let msg = msg.as_flatbuf();
            let min = msg.min_height();
            let max = msg.max_height();
            if json {
                println!(
                    "{}",
                    json!({
                        "min": min,
                        "max": max,
                    })
                );
            } else {
                println!("min: {}", min);
                println!("max: {}", max);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(debug_assertions)]
const DEFAULT_LOG_FILTER: &str = "solarxr_cli=trace,solarxr_client=trace";

#[cfg(not(debug_assertions))]
const DEFAULT_LOG_FILTER: &str = "solarxr_cli=info,solarxr_client=info";

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
                    eprintln!("Error: {err:?}");
                    ExitCode::FAILURE
                }
                Ok(exit_code) => exit_code,
            }
        })
}
