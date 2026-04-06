use std::collections::HashMap;
use std::fs;
use std::io;
use std::ops::Deref;
use std::path::Path;
use std::time::Duration;

use eyre::{Context, Result};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ActionBinding {
    pub left: Option<String>,
    pub right: Option<String>,
    pub double_click: Option<bool>,
    pub triple_click: Option<bool>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ActionProfile {
    pub reset_yaw: Option<ActionBinding>,
    pub reset_full: Option<ActionBinding>,
    pub reset_mounting: Option<ActionBinding>,
    pub reset_mounting_feet: Option<ActionBinding>,
    pub tracking_pause_toggle: Option<ActionBinding>,
    pub tracking_pause: Option<ActionBinding>,
    pub tracking_unpause: Option<ActionBinding>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ActionProfiles(HashMap<String, ActionProfile>);

impl Deref for ActionProfiles {
    type Target = HashMap<String, ActionProfile>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Delays {
    pub yaw: Duration,
    pub full: Duration,
    pub mounting: Duration,
    pub mounting_feet: Duration,
}

impl Default for Delays {
    fn default() -> Self {
        Self {
            yaw: Duration::ZERO,
            full: Duration::from_secs(3),
            mounting: Duration::from_secs(3),
            mounting_feet: Duration::from_secs(3),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Config {
    #[serde(default)]
    pub delays: Delays,
    pub action_profiles: ActionProfiles,
}

pub fn find() -> Result<Config> {
    let xdg = xdg::BaseDirectories::new();
    const CONFIG_PATH: &str = "solarxr-input/config.json";
    let f = || -> Result<io::BufReader<fs::File>> {
        let f = xdg
            .find_config_file(CONFIG_PATH)
            .ok_or(io::Error::from(io::ErrorKind::NotFound))?;
        Ok(io::BufReader::new(fs::File::open(f)?))
    }()
    .wrap_err("can't read config")?;
    serde_json::from_reader::<_, Config>(f).wrap_err("can't deserialize config")
}

pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Config> {
    let f = io::BufReader::new(fs::File::open(path)?);
    serde_json::from_reader::<_, Config>(f).wrap_err("can't deserialize config")
}
