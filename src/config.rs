use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub capture: CaptureConfig,
    pub vad: VadConfig,
    pub output: OutputConfig,
    pub targets: TargetsConfig,
    pub storage: StorageConfig,
    pub transcription: TranscriptionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    pub pre_speech_buffer_secs: f32,
    pub silence_threshold_secs: f32,
    pub speech_threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub directory: PathBuf,
    pub max_file_duration_mins: u32,
    pub organize_by_date: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetsConfig {
    pub processes: Vec<String>,
    pub mic_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub retention_days: u32,
    pub cleanup_interval_hours: u32,
    pub max_disk_usage_gb: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionConfig {
    pub backend: String,
    pub model: String,
    pub azure: AzureConfig,
    pub idle_watch: IdleWatchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AzureConfig {
    pub endpoint: String,
    pub api_key: String,
    pub deployment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleWatchConfig {
    pub cpu_threshold_percent: f32,
    pub idle_check_interval_secs: u64,
}

// --- Default implementations ---

impl Default for Config {
    fn default() -> Self {
        Self {
            capture: CaptureConfig::default(),
            vad: VadConfig::default(),
            output: OutputConfig::default(),
            targets: TargetsConfig::default(),
            storage: StorageConfig::default(),
            transcription: TranscriptionConfig::default(),
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            bit_depth: 16,
            channels: 1,
        }
    }
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            pre_speech_buffer_secs: 5.0,
            silence_threshold_secs: 3.0,
            speech_threshold: 0.5,
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        let directory = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("deskmic")
            .join("recordings");
        Self {
            directory,
            max_file_duration_mins: 30,
            organize_by_date: true,
        }
    }
}

impl Default for TargetsConfig {
    fn default() -> Self {
        Self {
            processes: vec!["ms-teams.exe".to_string()],
            mic_enabled: true,
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            retention_days: 30,
            cleanup_interval_hours: 6,
            max_disk_usage_gb: None,
        }
    }
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            backend: "local".to_string(),
            model: "base.en".to_string(),
            azure: AzureConfig::default(),
            idle_watch: IdleWatchConfig::default(),
        }
    }
}

impl Default for AzureConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            api_key: String::new(),
            deployment: String::new(),
        }
    }
}

impl Default for IdleWatchConfig {
    fn default() -> Self {
        Self {
            cpu_threshold_percent: 20.0,
            idle_check_interval_secs: 30,
        }
    }
}

// --- Config loading ---

impl Config {
    /// Load config from an explicit path, or search standard locations, or fall back to defaults.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        // 1. Check explicit path
        if let Some(p) = path {
            let content = std::fs::read_to_string(p).map_err(|e| {
                anyhow::anyhow!("Failed to read config file {}: {}", p.display(), e)
            })?;
            let config: Config = toml::from_str(&content)?;
            return Ok(config);
        }

        // 2. Check beside the executable
        if let Ok(exe_path) = std::env::current_exe() {
            let beside_exe = exe_path.parent().map(|p| p.join("deskmic.toml"));
            if let Some(p) = beside_exe {
                if p.exists() {
                    let content = std::fs::read_to_string(&p)?;
                    let config: Config = toml::from_str(&content)?;
                    return Ok(config);
                }
            }
        }

        // 3. Check platform config directory (e.g. %APPDATA%\deskmic\config.toml)
        if let Some(config_dir) = dirs::config_dir() {
            let platform_config = config_dir.join("deskmic").join("config.toml");
            if platform_config.exists() {
                let content = std::fs::read_to_string(&platform_config)?;
                let config: Config = toml::from_str(&content)?;
                return Ok(config);
            }
        }

        // 4. Fall back to defaults
        tracing::info!("No config file found, using defaults");
        Ok(Config::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = Config::default();
        assert_eq!(config.capture.sample_rate, 16000);
        assert_eq!(config.capture.bit_depth, 16);
        assert_eq!(config.capture.channels, 1);
        assert_eq!(config.vad.speech_threshold, 0.5);
        assert_eq!(config.vad.pre_speech_buffer_secs, 5.0);
        assert_eq!(config.vad.silence_threshold_secs, 3.0);
        assert_eq!(config.storage.retention_days, 30);
        assert_eq!(config.storage.cleanup_interval_hours, 6);
        assert!(config.storage.max_disk_usage_gb.is_none());
        assert_eq!(config.output.max_file_duration_mins, 30);
        assert!(config.output.organize_by_date);
        assert!(config.targets.mic_enabled);
        assert_eq!(config.targets.processes, vec!["ms-teams.exe"]);
        assert_eq!(config.transcription.backend, "local");
        assert_eq!(config.transcription.model, "base.en");
    }

    #[test]
    fn test_parse_toml_config() {
        let toml_str = r#"
            [capture]
            sample_rate = 48000

            [vad]
            speech_threshold = 0.8
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.capture.sample_rate, 48000);
        assert_eq!(config.vad.speech_threshold, 0.8);
        // Defaults still applied for unspecified fields
        assert_eq!(config.capture.bit_depth, 16);
        assert_eq!(config.capture.channels, 1);
        assert_eq!(config.storage.retention_days, 30);
        assert_eq!(config.output.max_file_duration_mins, 30);
    }

    #[test]
    fn test_parse_full_toml_config() {
        let toml_str = r#"
            [capture]
            sample_rate = 44100
            bit_depth = 24
            channels = 2

            [vad]
            pre_speech_buffer_secs = 3.0
            silence_threshold_secs = 2.0
            speech_threshold = 0.6

            [output]
            directory = "/tmp/deskmic"
            max_file_duration_mins = 60
            organize_by_date = false

            [targets]
            processes = ["zoom.exe", "slack.exe"]
            mic_enabled = false

            [storage]
            retention_days = 7
            cleanup_interval_hours = 12
            max_disk_usage_gb = 50.0

            [transcription]
            backend = "azure"
            model = "large-v3"

            [transcription.azure]
            endpoint = "https://example.openai.azure.com"
            api_key = "test-key"
            deployment = "whisper-large"

            [transcription.idle_watch]
            cpu_threshold_percent = 10.0
            idle_check_interval_secs = 60
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.capture.sample_rate, 44100);
        assert_eq!(config.capture.bit_depth, 24);
        assert_eq!(config.capture.channels, 2);
        assert_eq!(config.vad.pre_speech_buffer_secs, 3.0);
        assert!(!config.output.organize_by_date);
        assert_eq!(config.targets.processes, vec!["zoom.exe", "slack.exe"]);
        assert!(!config.targets.mic_enabled);
        assert_eq!(config.storage.retention_days, 7);
        assert_eq!(config.storage.max_disk_usage_gb, Some(50.0));
        assert_eq!(config.transcription.backend, "azure");
        assert_eq!(
            config.transcription.azure.endpoint,
            "https://example.openai.azure.com"
        );
        assert_eq!(config.transcription.idle_watch.cpu_threshold_percent, 10.0);
    }

    #[test]
    fn test_config_roundtrip_serialize() {
        let config = Config::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.capture.sample_rate, config.capture.sample_rate);
        assert_eq!(parsed.vad.speech_threshold, config.vad.speech_threshold);
        assert_eq!(parsed.storage.retention_days, config.storage.retention_days);
    }

    #[test]
    fn test_load_returns_defaults_when_no_file() {
        let config = Config::load(None).unwrap();
        assert_eq!(config.capture.sample_rate, 16000);
    }

    #[test]
    fn test_load_nonexistent_path_errors() {
        let result = Config::load(Some(Path::new("/nonexistent/config.toml")));
        assert!(result.is_err());
    }
}
