use crate::config::AppConfig;
use anyhow::Result;
use figment::{
    providers::{Env, Format, Json, Toml},
    Figment,
};

pub struct ConfigLoader;

impl ConfigLoader {
    /// Loads application configuration by merging TOML, environment variables, and JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if configuration files cannot be read or parsed.
    pub fn load() -> Result<AppConfig> {
        let config: AppConfig = Figment::new()
            .merge(Toml::file("config/Config.toml"))
            .merge(Env::prefixed("APP_"))
            .join(Json::file("config/Config.json"))
            .extract()?;

        Ok(config)
    }

    /// Loads application configuration with a specific profile.
    ///
    /// # Errors
    ///
    /// Returns an error if configuration files cannot be read or parsed.
    pub fn load_with_profile(profile: &str) -> Result<AppConfig> {
        let config: AppConfig = Figment::new()
            .merge(Toml::file("config/Config.toml"))
            .merge(Toml::file(format!("config/Config.{profile}.toml")))
            .merge(Env::prefixed("APP_"))
            .join(Json::file("config/Config.json"))
            .extract()?;

        Ok(config)
    }
}
