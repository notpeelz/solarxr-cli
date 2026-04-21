use std::collections::HashMap;
use std::env;
use std::io;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eyre::{Result, bail};
use openxr as xr;
use paste::paste;
use solarxr_client::SolarXRClient;
use solarxr_client::SolarXRError;
use solarxr_client::proto;
use tracing::{error, trace};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod cli;
mod config;

const CLICK_TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Default)]
struct ActionBinding {
    left: Option<xr::Path>,
    right: Option<xr::Path>,
    double_click: bool,
    triple_click: bool,
}

#[derive(Default)]
struct BoundAction {
    binding: ActionBinding,
    click_count: usize,
    last_clicked: Option<Instant>,
}

#[cfg(debug_assertions)]
const DEFAULT_LOG_FILTER: &str = "solarxr_input=trace,solarxr_client=trace";

#[cfg(not(debug_assertions))]
const DEFAULT_LOG_FILTER: &str = "solarxr_input=info,solarxr_client=info";

fn main() -> ExitCode {
    let directives = env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LOG_FILTER.to_owned());
    let env_filter = EnvFilter::builder().parse_lossy(directives);

    let fmt_layer = tracing_subscriber::fmt::layer()
        .compact()
        .without_time()
        .with_writer(io::stderr);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
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

async fn exec() -> Result<ExitCode> {
    let args = <crate::cli::Args as clap::Parser>::parse();

    let cfg = if let Some(config_path) = args.config_path {
        config::from_path(config_path)?
    } else {
        config::find()?
    };

    let connect = async || -> Result<SolarXRClient, SolarXRError> {
        if let Some(socket_path) = args.socket_path {
            SolarXRClient::from_socket_path(socket_path).await
        } else {
            SolarXRClient::from_default_socket_paths().await
        }
    };

    let client = Arc::new(connect().await?);

    let entry = xr::Entry::linked();

    let available_extensions = entry.enumerate_extensions()?;
    let mut extensions = xr::ExtensionSet::default();

    if !available_extensions.mnd_headless {
        bail!("xr runtime doesn't support MND_headless");
    }
    extensions.mnd_headless = true;
    extensions.ext_hp_mixed_reality_controller =
        available_extensions.ext_hp_mixed_reality_controller;

    let instance = entry.create_instance(
        &xr::ApplicationInfo {
            application_name: "solarxr-input",
            application_version: 1,
            ..Default::default()
        },
        &extensions,
        &[],
    )?;
    let system = instance.system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)?;
    let (session, _, _) = unsafe {
        instance.create_session::<xr::Headless>(system, &xr::headless::SessionCreateInfo {})
    }?;

    let left_hand = instance.string_to_path("/user/hand/left")?;
    let right_hand = instance.string_to_path("/user/hand/right")?;
    let subaction_paths = [left_hand, right_hand];

    let action_set = instance.create_action_set("main", "Main Bindings", 0)?;
    let action_reset_yaw =
        action_set.create_action::<bool>("reset_yaw", "Yaw Reset", &subaction_paths)?;
    let action_reset_full =
        action_set.create_action::<bool>("reset_full", "Full Reset", &subaction_paths)?;
    let action_reset_mounting =
        action_set.create_action::<bool>("reset_mounting", "Mounting Reset", &subaction_paths)?;
    let action_reset_mounting_feet = action_set.create_action::<bool>(
        "reset_mounting_feet",
        "Feet Mounting Reset",
        &subaction_paths,
    )?;
    let action_tracking_pause =
        action_set.create_action::<bool>("tracking_pause", "Pause tracking", &subaction_paths)?;
    let action_tracking_unpause = action_set.create_action::<bool>(
        "tracking_unpause",
        "Unpause tracking",
        &subaction_paths,
    )?;
    let action_tracking_pause_toggle = action_set.create_action::<bool>(
        "tracking_pause_toggle",
        "Toggle Pause Tracking",
        &subaction_paths,
    )?;

    let instantiate_binding = |cfg: &config::ActionBinding| -> Result<ActionBinding> {
        let left = cfg
            .left
            .as_ref()
            .map(|s| instance.string_to_path(&s))
            .transpose()?;
        let right = cfg
            .right
            .as_ref()
            .map(|s| instance.string_to_path(&s))
            .transpose()?;

        let double_click = cfg.double_click.unwrap_or(false);
        let triple_click = cfg.triple_click.unwrap_or(false);
        if double_click && triple_click {
            bail!("binding can't have double_click and triple_click");
        }

        Ok(ActionBinding {
            left,
            right,
            double_click,
            triple_click,
        })
    };

    type ProfileBindingMap = HashMap<String, BoundAction>;
    #[derive(Default)]
    struct BoundActions {
        reset_yaw: ProfileBindingMap,
        reset_full: ProfileBindingMap,
        reset_mounting: ProfileBindingMap,
        reset_mounting_feet: ProfileBindingMap,
        tracking_pause: ProfileBindingMap,
        tracking_unpause: ProfileBindingMap,
        tracking_pause_toggle: ProfileBindingMap,
    }
    let mut bound_actions = BoundActions::default();

    for (profile_path, p) in &*cfg.action_profiles {
        let mut bindings = Vec::<xr::Binding>::new();
        macro_rules! instantiate {
            ($name:ident) => {
                paste! {
                    if let Some(binding) = p.$name.as_ref().map(instantiate_binding).transpose()? {
                        if let Some(left) = binding.left {
                            bindings.push(xr::Binding::new(&[<action_ $name>], left));
                        }

                        if let Some(right) = binding.right {
                            bindings.push(xr::Binding::new(&[<action_ $name>], right));
                        }

                        bound_actions.$name.insert(
                            profile_path.to_owned(),
                            BoundAction {
                                binding,
                                ..Default::default()
                            },
                        );
                    }
                }
            };
        }

        instantiate!(reset_yaw);
        instantiate!(reset_full);
        instantiate!(reset_mounting);
        instantiate!(reset_mounting_feet);
        instantiate!(tracking_pause);
        instantiate!(tracking_unpause);
        instantiate!(tracking_pause_toggle);

        if !bindings.is_empty() {
            instance.suggest_interaction_profile_bindings(
                instance.string_to_path(profile_path)?,
                &bindings,
            )?;
        }
    }
    session.attach_action_sets(&[&action_set])?;

    let mut event_storage = xr::EventDataBuffer::new();
    let mut session_running = false;
    let mut ticker = tokio::time::interval(Duration::from_millis(10));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        let now = Instant::now();

        while let Some(event) = instance.poll_event(&mut event_storage)? {
            match event {
                xr::Event::SessionStateChanged(e) => match e.state() {
                    xr::SessionState::IDLE => {
                        trace!("session state changed: IDLE");
                    }
                    xr::SessionState::READY => {
                        trace!("session state changed: READY");
                        session.begin(xr::ViewConfigurationType::PRIMARY_STEREO)?;
                    }
                    xr::SessionState::VISIBLE => {
                        trace!("session state changed: VISIBLE");
                        session_running = true;
                    }
                    xr::SessionState::SYNCHRONIZED => {
                        trace!("session state changed: SYNCHRONIZED");
                    }
                    xr::SessionState::FOCUSED => {
                        trace!("session state changed: FOCUSED");
                    }
                    xr::SessionState::STOPPING => {
                        trace!("session state changed: STOPPING");
                        session.end()?;
                        session_running = false;
                    }
                    xr::SessionState::LOSS_PENDING => {
                        trace!("session state changed: LOSS_PENDING");
                        return Ok(ExitCode::SUCCESS);
                    }
                    xr::SessionState::EXITING => {
                        trace!("session state changed: EXITING");
                        return Ok(ExitCode::SUCCESS);
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        if !session_running {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }

        session.sync_actions(&[xr::ActiveActionSet::new(&action_set)])?;

        for hand in [left_hand, right_hand] {
            let profile = session.current_interaction_profile(hand)?;
            if profile == xr::Path::NULL {
                continue;
            }
            let profile = instance.path_to_string(profile)?;

            fn check_action(
                hand: xr::Path,
                action: &xr::Action<bool>,
                bound_action: &mut BoundAction,
                session: &openxr::Session<openxr::Headless>,
                now: &Instant,
            ) -> Result<bool> {
                let state = action.state(session, hand)?;
                if state.current_state && state.changed_since_last_sync {
                    if bound_action
                        .last_clicked
                        .is_none_or(|x| (*now - x) <= CLICK_TIMEOUT)
                    {
                        bound_action.click_count += 1;
                    } else {
                        bound_action.click_count = 1;
                    }
                    bound_action.last_clicked = Some(*now);

                    let mut active = true;
                    if bound_action.binding.double_click {
                        active = bound_action.click_count == 2;
                    } else if bound_action.binding.triple_click {
                        active = bound_action.click_count == 3;
                    }

                    Ok(active)
                } else {
                    Ok(false)
                }
            }

            macro_rules! check_action {
                ($name:ident) => {
                    paste! {
                        bound_actions
                            .$name
                            .get_mut(&profile)
                            .map(|b| check_action(hand, &[<action_ $name>], b, &session, &now))
                            .transpose()?
                            .is_some()
                    }
                };
            }

            if check_action!(reset_yaw) {
                trace!("reset_yaw triggered");
                tokio::spawn({
                    let client = Arc::clone(&client);
                    async move {
                        if let Err(err) = client.reset_yaw(cfg.delays.yaw, true).await {
                            error!("reset_yaw failed: {err}");
                        }
                    }
                });
            }
            if check_action!(reset_full) {
                trace!("reset_full triggered");
                tokio::spawn({
                    let client = Arc::clone(&client);
                    async move {
                        if let Err(err) = client.reset_full(cfg.delays.full, true).await {
                            error!("reset_full failed: {err}");
                        }
                    }
                });
            }
            if check_action!(reset_mounting) {
                trace!("reset_mounting triggered");
                tokio::spawn({
                    let client = Arc::clone(&client);
                    async move {
                        if let Err(err) = client.reset_mounting(cfg.delays.mounting, true).await {
                            error!("reset_mounting failed: {err}");
                        }
                    }
                });
            }
            if check_action!(reset_mounting_feet) {
                trace!("reset_mounting_feet triggered");
                tokio::spawn({
                    let client = Arc::clone(&client);
                    async move {
                        if let Err(err) = client
                            .reset_mounting_with_parts(
                                &[
                                    proto::datatypes::BodyPart::LEFT_FOOT,
                                    proto::datatypes::BodyPart::RIGHT_FOOT,
                                ],
                                cfg.delays.mounting_feet,
                                true,
                            )
                            .await
                        {
                            error!("reset_mounting_feet failed: {err}");
                        }
                    }
                });
            }
            if check_action!(tracking_pause) {
                trace!("tracking_pause triggered");
                tokio::spawn({
                    let client = Arc::clone(&client);
                    async move {
                        if let Err(err) = client.set_pause_tracking(true).await {
                            error!("tracking_pause failed: {err}");
                        }
                    }
                });
            }
            if check_action!(tracking_unpause) {
                trace!("tracking_unpause triggered");
                tokio::spawn({
                    let client = Arc::clone(&client);
                    async move {
                        if let Err(err) = client.set_pause_tracking(false).await {
                            error!("tracking_unpause failed: {err}");
                        }
                    }
                });
            }
            if check_action!(tracking_pause_toggle) {
                trace!("tracking_pause_toggle triggered");
                tokio::spawn({
                    let client = Arc::clone(&client);
                    async move {
                        let toggle_pause = async move || -> io::Result<()> {
                            let paused = client.pause_tracking_state().await?;
                            client.set_pause_tracking(!paused).await?;
                            Ok(())
                        };
                        if let Err(err) = toggle_pause().await {
                            error!("tracking_pause_toggle failed: {err}");
                        }
                    }
                });
            }
        }
    }
}
