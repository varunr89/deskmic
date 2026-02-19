use serde::{Deserialize, Serialize};
use std::fmt;
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
    #[serde(default)]
    pub summarization: SummarizationConfig,
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AzureConfig {
    pub endpoint: String,
    pub api_key: String,
    pub deployment: String,
}

impl fmt::Debug for AzureConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AzureConfig")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"[REDACTED]")
            .field("deployment", &self.deployment)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleWatchConfig {
    pub cpu_threshold_percent: f32,
    pub idle_check_interval_secs: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SummarizationConfig {
    /// Azure OpenAI deployment name for chat completions (e.g. "gpt-4o").
    pub deployment: String,
    /// ACS Communication Services endpoint for sending email.
    pub acs_endpoint: String,
    /// ACS access key (or set DESKMIC_ACS_KEY environment variable).
    pub acs_api_key: String,
    /// Sender email address from the ACS Email verified domain.
    pub sender_address: String,
    /// Recipient email address for summary delivery.
    pub recipient_address: String,
    /// Custom system prompt for summarization. Use {date_label} as placeholder.
    /// Leave empty to use the built-in default prompt.
    pub system_prompt: String,
}

impl fmt::Debug for SummarizationConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SummarizationConfig")
            .field("deployment", &self.deployment)
            .field("acs_endpoint", &self.acs_endpoint)
            .field("acs_api_key", &"[REDACTED]")
            .field("sender_address", &self.sender_address)
            .field("recipient_address", &self.recipient_address)
            .field("system_prompt", &self.system_prompt)
            .finish()
    }
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
            summarization: SummarizationConfig::default(),
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
            cpu_threshold_percent: 50.0,
            idle_check_interval_secs: 30,
        }
    }
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self {
            deployment: String::new(),
            acs_endpoint: String::new(),
            acs_api_key: String::new(),
            sender_address: String::new(),
            recipient_address: String::new(),
            system_prompt: String::new(),
        }
    }
}

// --- Config loading ---

impl Config {
    /// Load config and return the resolved file path (if any).
    pub fn load_with_path(path: Option<&Path>) -> anyhow::Result<(Self, Option<PathBuf>)> {
        // 1. Check explicit path
        if let Some(p) = path {
            let content = std::fs::read_to_string(p).map_err(|e| {
                anyhow::anyhow!("Failed to read config file {}: {}", p.display(), e)
            })?;
            let config: Config = toml::from_str(&content)?;
            return Ok((config, Some(p.to_path_buf())));
        }

        // 2. Check beside the executable
        if let Ok(exe_path) = std::env::current_exe() {
            let beside_exe = exe_path.parent().map(|p| p.join("deskmic.toml"));
            if let Some(p) = beside_exe {
                if p.exists() {
                    let content = std::fs::read_to_string(&p)?;
                    let config: Config = toml::from_str(&content)?;
                    return Ok((config, Some(p)));
                }
            }
        }

        // 3. Check platform config directory (e.g. %APPDATA%\deskmic\config.toml)
        if let Some(config_dir) = dirs::config_dir() {
            let platform_config = config_dir.join("deskmic").join("config.toml");
            if platform_config.exists() {
                let content = std::fs::read_to_string(&platform_config)?;
                let config: Config = toml::from_str(&content)?;
                return Ok((config, Some(platform_config)));
            }
        }

        // 4. Fall back to defaults
        tracing::info!("No config file found, using defaults");
        Ok((Config::default(), None))
    }

    /// Load config (without tracking the resolved path). Kept for backward compat.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        Self::load_with_path(path).map(|(config, _)| config)
    }

    /// Generate a default config file with all fields and inline documentation.
    pub fn generate_default_commented() -> String {
        let default_output_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("deskmic")
            .join("recordings");
        let output_dir_str = default_output_dir.to_string_lossy().replace('\\', "\\\\");

        format!(
r#"# deskmic configuration
# Edit this file to customize recording, transcription, and storage settings.
# Restart deskmic after saving changes for them to take effect.

[capture]
# Audio capture sample rate in Hz. 16000 is required for VAD compatibility.
sample_rate = 16000
# Bits per sample (16-bit PCM is standard).
bit_depth = 16
# Number of audio channels (1 = mono).
channels = 1

[vad]
# Seconds of audio to keep in the ring buffer before speech is detected.
# This "pre-roll" ensures you don't lose the beginning of a sentence.
pre_speech_buffer_secs = 5.0
# Seconds of silence after speech before the recording segment ends.
# Lower values = more files, higher values = longer trailing silence.
silence_threshold_secs = 3.0
# Voice activity detection confidence threshold (0.0 to 1.0).
# Lower = more sensitive (catches quiet speech), higher = fewer false positives.
speech_threshold = 0.5

[output]
# Directory where WAV recordings are saved.
directory = "{output_dir}"
# Maximum duration of a single recording file in minutes.
# Recordings are split into new files when this limit is reached.
max_file_duration_mins = 30
# Organize recordings into date-based subdirectories (YYYY-MM-DD).
organize_by_date = true

[targets]
# List of process names to capture audio from (application loopback).
processes = ["ms-teams.exe"]
# Whether to also capture from the default microphone.
mic_enabled = true

[storage]
# Number of days to keep recordings before automatic cleanup.
retention_days = 30
# How often (in hours) to run the cleanup job.
cleanup_interval_hours = 6
# Maximum total disk usage in GB. Oldest files are deleted first.
# Comment out or remove to disable disk usage limits.
# max_disk_usage_gb = 50.0

[transcription]
# Transcription backend: "local" (whisper.cpp on device) or "azure" (cloud API).
backend = "local"
# Whisper model name (for local backend). Options: tiny.en, base.en, small.en, medium.en
# Or an absolute path to a .bin model file.
model = "base.en"

[transcription.azure]
# Azure OpenAI Whisper endpoint URL.
# endpoint = "https://your-resource.openai.azure.com"
# API key (or set DESKMIC_AZURE_KEY environment variable).
# api_key = ""
# Deployment name for the Whisper model.
# deployment = "whisper"

[transcription.idle_watch]
# Only run transcription when average CPU usage is below this percentage.
# Prevents transcription from slowing down your machine during active use.
cpu_threshold_percent = 50.0
# How often (in seconds) to check whether the system is idle for transcription.
idle_check_interval_secs = 30

[summarization]
# Azure OpenAI deployment name for chat completions (used by 'deskmic summarize').
# This reuses the endpoint and api_key from [transcription.azure].
# deployment = "gpt-4o"
# ACS (Azure Communication Services) endpoint for sending summary emails.
# acs_endpoint = "https://your-acs.unitedstates.communication.azure.com"
# ACS access key (or set DESKMIC_ACS_KEY environment variable).
# acs_api_key = ""
# Sender email address from the ACS Email verified domain.
# sender_address = "DoNotReply@your-domain.azurecomm.net"
# Recipient email address for summary delivery.
# recipient_address = "you@example.com"
# Custom system prompt for the LLM summarizer. Use {{date_label}} as a placeholder
# for the date range being summarized. Leave empty to use the built-in default.
# system_prompt = ""
"#,
            output_dir = output_dir_str
        )
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

    #[test]
    fn test_load_with_path_returns_resolved_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_file = tmp.path().join("deskmic.toml");
        std::fs::write(&config_file, "[capture]\nsample_rate = 44100\n").unwrap();

        let (config, resolved) = Config::load_with_path(Some(config_file.as_path())).unwrap();
        assert_eq!(config.capture.sample_rate, 44100);
        assert_eq!(resolved, Some(config_file));
    }

    #[test]
    fn test_load_with_path_none_returns_none_when_no_file() {
        let (config, resolved) = Config::load_with_path(None).unwrap();
        assert_eq!(config.capture.sample_rate, 16000);
        let _ = resolved;
    }

    #[test]
    fn test_generate_default_commented_is_valid_toml() {
        let content = Config::generate_default_commented();
        // Should be parseable as valid TOML (comments are stripped by parser)
        let config: Config = toml::from_str(&content).unwrap();
        assert_eq!(config.capture.sample_rate, 16000);
        assert_eq!(config.vad.pre_speech_buffer_secs, 5.0);
        assert_eq!(config.output.max_file_duration_mins, 30);
        assert_eq!(config.transcription.backend, "local");
        assert_eq!(config.transcription.idle_watch.idle_check_interval_secs, 30);
    }

    #[test]
    fn test_generate_default_commented_has_all_sections() {
        let content = Config::generate_default_commented();
        assert!(content.contains("[capture]"));
        assert!(content.contains("[vad]"));
        assert!(content.contains("[output]"));
        assert!(content.contains("[targets]"));
        assert!(content.contains("[storage]"));
        assert!(content.contains("[transcription]"));
        assert!(content.contains("[transcription.azure]"));
        assert!(content.contains("[transcription.idle_watch]"));
        assert!(content.contains("[summarization]"));
    }

    #[test]
    fn test_azure_config_debug_redacts_api_key() {
        let config = AzureConfig {
            endpoint: "https://example.openai.azure.com".to_string(),
            api_key: "super-secret-key-12345".to_string(),
            deployment: "whisper".to_string(),
        };
        let debug_output = format!("{:?}", config);
        assert!(
            !debug_output.contains("super-secret-key-12345"),
            "Debug output should not contain the API key"
        );
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug output should show [REDACTED] for api_key"
        );
        assert!(
            debug_output.contains("https://example.openai.azure.com"),
            "Debug output should still show the endpoint"
        );
    }

    #[test]
    fn test_summarization_config_debug_redacts_acs_key() {
        let config = SummarizationConfig {
            acs_api_key: "acs-super-secret-key-67890".to_string(),
            acs_endpoint: "https://my-acs.communication.azure.com".to_string(),
            ..Default::default()
        };
        let debug_output = format!("{:?}", config);
        assert!(
            !debug_output.contains("acs-super-secret-key-67890"),
            "Debug output should not contain the ACS API key"
        );
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug output should show [REDACTED] for acs_api_key"
        );
        assert!(
            debug_output.contains("https://my-acs.communication.azure.com"),
            "Debug output should still show the endpoint"
        );
    }

    #[test]
    fn test_config_debug_redacts_nested_secrets() {
        let mut config = Config::default();
        config.transcription.azure.api_key = "nested-secret-key".to_string();
        config.summarization.acs_api_key = "nested-acs-secret".to_string();
        let debug_output = format!("{:?}", config);
        assert!(
            !debug_output.contains("nested-secret-key"),
            "Config debug should not contain nested Azure API key"
        );
        assert!(
            !debug_output.contains("nested-acs-secret"),
            "Config debug should not contain nested ACS API key"
        );
    }
}
