use mlua::{Lua, LuaSerdeExt};
use serde::Deserialize;
use std::{fs, path::PathBuf, sync::Arc};

#[derive(Deserialize)]
pub struct FullConfig {
    pub general: MoxidleConfig,
    pub timeouts: Vec<TimeoutConfig>,
}

impl FullConfig {
    pub fn load() -> Result<(Self, PathBuf), Box<dyn std::error::Error>> {
        let config_path = get_config_path()?;
        let lua_code = fs::read_to_string(&config_path)?;
        let lua = Lua::new();
        let lua_result = lua.load(&lua_code).eval()?;
        Ok((lua.from_value(lua_result)?, config_path))
    }

    pub fn apply(self) -> (MoxidleConfig, Vec<TimeoutConfig>) {
        (self.general, self.timeouts)
    }
}

pub fn get_config_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".config")))?;

    Ok(config_dir.join("moxidle/config.lua"))
}

#[derive(Deserialize)]
pub struct MoxidleConfig {
    pub lock_cmd: Option<Arc<str>>,
    pub unlock_cmd: Option<Arc<str>>,
    pub before_sleep_cmd: Option<Arc<str>>,
    pub after_sleep_cmd: Option<Arc<str>>,
    pub ignore_systemd_inhibit: bool,
    pub ignore_audio_inhibit: bool,
    pub ignore_dbus_inhibit: bool,
}

#[derive(Deserialize)]
pub struct TimeoutConfig {
    pub timeout: u32,
    pub on_timeout: Option<Arc<str>>,
    pub on_resume: Option<Arc<str>>,
}

impl TimeoutConfig {
    pub fn timeout(&self) -> u32 {
        self.timeout * 1000
    }
}
