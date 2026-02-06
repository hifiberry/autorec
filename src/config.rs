use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::io;

/// Configuration defaults that can be saved to a file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<u32>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<usize>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<f64>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_range: Option<f64>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_db: Option<f64>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub off_threshold: Option<f64>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub silence_duration: Option<f64>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_length: Option<f64>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_vumeter: Option<bool>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_keyboard: Option<bool>,
}

impl Config {
    /// Create a new empty config
    pub fn new() -> Self {
        Config {
            source: None,
            rate: None,
            channels: None,
            format: None,
            interval: None,
            db_range: None,
            max_db: None,
            off_threshold: None,
            silence_duration: None,
            min_length: None,
            no_vumeter: None,
            no_keyboard: None,
        }
    }

    /// Get the config file path (~/.state/autorec/defaults.toml)
    pub fn get_config_path() -> Result<PathBuf, io::Error> {
        let home = std::env::var("HOME")
            .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "HOME environment variable not set"))?;
        
        let config_dir = Path::new(&home).join(".state").join("autorec");
        Ok(config_dir.join("defaults.toml"))
    }

    /// Load config from file
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let config_path = Self::get_config_path()?;
        
        if !config_path.exists() {
            // Return empty config if file doesn't exist
            return Ok(Config::new());
        }

        let content = fs::read_to_string(&config_path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save config to file
    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = Self::get_config_path()?;
        
        // Create parent directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let toml_string = toml::to_string_pretty(self)?;
        fs::write(&config_path, toml_string)?;
        
        Ok(())
    }

    /// Merge this config with another, preferring values from other
    pub fn merge(&mut self, other: &Config) {
        if other.source.is_some() {
            self.source = other.source.clone();
        }
        if other.rate.is_some() {
            self.rate = other.rate;
        }
        if other.channels.is_some() {
            self.channels = other.channels;
        }
        if other.format.is_some() {
            self.format = other.format.clone();
        }
        if other.interval.is_some() {
            self.interval = other.interval;
        }
        if other.db_range.is_some() {
            self.db_range = other.db_range;
        }
        if other.max_db.is_some() {
            self.max_db = other.max_db;
        }
        if other.off_threshold.is_some() {
            self.off_threshold = other.off_threshold;
        }
        if other.silence_duration.is_some() {
            self.silence_duration = other.silence_duration;
        }
        if other.min_length.is_some() {
            self.min_length = other.min_length;
        }
        if other.no_vumeter.is_some() {
            self.no_vumeter = other.no_vumeter;
        }
        if other.no_keyboard.is_some() {
            self.no_keyboard = other.no_keyboard;
        }
    }

    /// Print the config in a human-readable format
    pub fn print(&self, title: &str) {
        println!("{}:", title);
        
        if let Some(source) = &self.source {
            println!("  Audio source:       {}", source);
        }
        if let Some(rate) = self.rate {
            println!("  Sample rate:        {} Hz", rate);
        }
        if let Some(channels) = self.channels {
            println!("  Channels:           {}", channels);
        }
        if let Some(format) = &self.format {
            println!("  Format:             {}", format);
        }
        if let Some(interval) = self.interval {
            println!("  Update interval:    {} seconds", interval);
        }
        if let Some(db_range) = self.db_range {
            println!("  dB range:           {} dB", db_range);
        }
        if let Some(max_db) = self.max_db {
            println!("  Maximum dB:         {} dB", max_db);
        }
        if let Some(off_threshold) = self.off_threshold {
            println!("  Off threshold:      {} dB", off_threshold);
        }
        if let Some(silence_duration) = self.silence_duration {
            println!("  Silence duration:   {} seconds", silence_duration);
        }
        if let Some(min_length) = self.min_length {
            println!("  Min recording:      {} seconds", min_length);
        }
        if let Some(no_vumeter) = self.no_vumeter {
            println!("  VU meter:           {}", if no_vumeter { "disabled" } else { "enabled" });
        }
        if let Some(no_keyboard) = self.no_keyboard {
            println!("  Keyboard shortcuts: {}", if no_keyboard { "disabled" } else { "enabled" });
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}
