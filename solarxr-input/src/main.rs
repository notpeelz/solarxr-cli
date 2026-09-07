use std::collections::HashMap;
use std::env;
use std::io;
use std::os::raw::c_int;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::Once;
use std::time::{Duration, Instant};

use eyre::Result;
use openxr as xr;
use paste::paste;
use solarxr_client::SolarXRClient;
use solarxr_client::SolarXRError;
use solarxr_client::proto;
use tracing::{error, trace, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod cli;
mod config;

const CLICK_TIMEOUT: Duration = Duration::from_millis(300);
const WAIT_INTERVAL: Duration = Duration::from_secs(1);

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

struct OpenXRState {
    instance: xr::Instance,
    session: xr::Session<xr::Headless>,
    left_hand: xr::Path,
    right_hand: xr::Path,
    action_set: xr::ActionSet,
    action_reset_yaw: xr::Action<bool>,
    action_reset_full: xr::Action<bool>,
    action_reset_mounting: xr::Action<bool>,
    action_reset_mounting_feet: xr::Action<bool>,
    action_tracking_pause: xr::Action<bool>,
    action_tracking_unpause: xr::Action<bool>,
    action_tracking_pause_toggle: xr::Action<bool>,
    bound_actions: BoundActions,
}

fn is_retryable(err: xr::sys::Result) -> bool {
    matches!(
        err,
        xr::sys::Result::ERROR_RUNTIME_UNAVAILABLE
            | xr::sys::Result::ERROR_INITIALIZATION_FAILED
            | xr::sys::Result::ERROR_FORM_FACTOR_UNAVAILABLE
    )
}

struct SilenceStderr {
    saved: c_int,
    dev_null: c_int,
}

impl SilenceStderr {
    fn new() -> Self {
        unsafe {
            let saved = libc::dup(libc::STDERR_FILENO);
            let dev_null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
            libc::dup2(dev_null, libc::STDERR_FILENO);
            SilenceStderr { saved, dev_null }
        }
    }
}

impl Drop for SilenceStderr {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved, libc::STDERR_FILENO);
            libc::close(self.saved);
            libc::close(self.dev_null);
        }
    }
}

fn init_openxr(cfg: &config::Config) -> openxr::Result<OpenXRState> {
    let entry = xr::Entry::linked();

    let available_extensions = entry.enumerate_extensions()?;
    let mut extensions = xr::ExtensionSet::default();

    if !available_extensions.mnd_headless {
        return Err(xr::sys::Result::ERROR_EXTENSION_NOT_PRESENT);
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

    let instantiate_binding = |cfg: &config::ActionBinding| -> openxr::Result<ActionBinding> {
        let left = cfg
            .left
            .as_ref()
            .map(|s| instance.string_to_path(s))
            .transpose()?;
        let right = cfg
            .right
            .as_ref()
            .map(|s| instance.string_to_path(s))
            .transpose()?;

        let double_click = cfg.double_click.unwrap_or(false);
        let triple_click = cfg.triple_click.unwrap_or(false);
        if double_click && triple_click {
            return Err(xr::sys::Result::ERROR_VALIDATION_FAILURE);
        }

        Ok(ActionBinding {
            left,
            right,
            double_click,
            triple_click,
        })
    };

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

    Ok(OpenXRState {
        instance,
        session,
        left_hand,
        right_hand,
        action_set,
        action_reset_yaw,
        action_reset_full,
        action_reset_mounting,
        action_reset_mounting_feet,
        action_tracking_pause,
        action_tracking_unpause,
        action_tracking_pause_toggle,
        bound_actions,
    })
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

    if args.wait_xr {
        tokio::spawn(async {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to install SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = sigterm.recv() => {}
            }
            std::process::exit(0);
        });
    }

    let mut attempts = 0_usize;
    let mut state = loop {
        attempts += 1;
        let result = if attempts > 1 {
            let _silence = SilenceStderr::new();
            init_openxr(&cfg)
        } else {
            init_openxr(&cfg)
        };
        match result {
            Ok(state) => break state,
            Err(err) => {
                if !args.wait_xr || !is_retryable(err) {
                    return Err(err.into());
                }
                static WARN: Once = Once::new();
                WARN.call_once(|| {
                    warn!("XR runtime not available: {err}. Waiting for it to become ready.");
                });
                tokio::time::sleep(WAIT_INTERVAL).await;
            }
        }
    };

    let mut event_storage = xr::EventDataBuffer::new();
    let mut session_running = false;
    let mut ticker = tokio::time::interval(Duration::from_millis(10));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        let now = Instant::now();

        while let Some(event) = state.instance.poll_event(&mut event_storage)? {
            #[allow(clippy::single_match)]
            match event {
                xr::Event::SessionStateChanged(e) => match e.state() {
                    xr::SessionState::IDLE => {
                        trace!("session state changed: IDLE");
                    }
                    xr::SessionState::READY => {
                        trace!("session state changed: READY");
                        state
                            .session
                            .begin(xr::ViewConfigurationType::PRIMARY_STEREO)?;
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
                        state.session.end()?;
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

        state
            .session
            .sync_actions(&[xr::ActiveActionSet::new(&state.action_set)])?;

        for hand in [state.left_hand, state.right_hand] {
            let profile = state.session.current_interaction_profile(hand)?;
            if profile == xr::Path::NULL {
                continue;
            }
            let profile = state.instance.path_to_string(profile)?;

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
                        state
                            .bound_actions
                            .$name
                            .get_mut(&profile)
                            .map(|b| check_action(hand, &state.[<action_ $name>], b, &state.session, &now))
                            .transpose()?
                            .unwrap_or(false)
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
