use mlua::{Lua, LuaSerdeExt};
use serde::Deserialize;
use std::{fs, path::PathBuf, rc::Rc, sync::Arc};

#[derive(Deserialize)]
pub struct Config {
    pub general: MoxidleConfig,
    pub timeouts: Vec<TimeoutConfig>,
}

impl Config {
    pub fn load(
        path: Option<PathBuf>,
    ) -> Result<(MoxidleConfig, Vec<TimeoutConfig>), Box<dyn std::error::Error>> {
        let config_path = if let Some(path) = path {
            path
        } else {
            Self::path()?
        };
        let lua_code = fs::read_to_string(&config_path)?;
        let lua = Lua::new();
        let lua_result = lua.load(&lua_code).eval()?;

        let config: Config = lua.from_value(lua_result)?;

        Ok((config.general, config.timeouts))
    }

    pub fn path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".config")))?;

        Ok(config_dir.join("moxidle/config.lua"))
    }
}

#[derive(Deserialize)]
pub struct MoxidleConfig {
    pub lock_cmd: Option<Arc<str>>,
    pub unlock_cmd: Option<Arc<str>>,
    pub before_sleep_cmd: Option<Arc<str>>,
    pub after_sleep_cmd: Option<Arc<str>>,
    #[serde(default)]
    pub ignore_dbus_inhibit: bool,
    #[serde(default)]
    pub ignore_systemd_inhibit: bool,
    #[serde(default)]
    pub ignore_audio_inhibit: bool,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    OnBattery,
    OnAc,
    BatteryBelow(f64),
    BatteryAbove(f64),
}

#[derive(Deserialize)]
pub struct TimeoutConfig {
    #[serde(default)]
    pub conditions: Rc<[Condition]>,
    pub timeout: u32,
    pub on_timeout: Option<Arc<str>>,
    pub on_resume: Option<Arc<str>>,
}

impl TimeoutConfig {
    pub fn timeout_millis(&self) -> u32 {
        self.timeout * 1000
    }
}
