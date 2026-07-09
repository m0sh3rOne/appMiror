use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub pin: String,
    pub locked_apps: Vec<String>,
    pub launcher_locked: bool,
    pub server_port: u16,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            pin: "1234".to_string(),
            locked_apps: Vec::new(),
            launcher_locked: false,
            server_port: 8080,
        }
    }
}

impl AppConfig {
    pub fn get_path() -> PathBuf {
        if let Some(mut path) = dirs::config_dir() {
            path.push("appMirror");
            let _ = fs::create_dir_all(&path);
            path.push("config.json");
            path
        } else {
            PathBuf::from("config.json")
        }
    }

    pub fn load() -> Self {
        let path = Self::get_path();
        if path.exists() {
            if let Ok(data) = fs::read_to_string(path) {
                if let Ok(config) = serde_json::from_str::<AppConfig>(&data) {
                    return config;
                }
            }
        }
        
        // Save default if it doesn't exist
        let config = Self::default();
        let _ = config.save();
        config
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::get_path();
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)?;
        Ok(())
    }
}
