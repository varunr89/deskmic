pub mod azure_openai;
pub mod backend;
pub mod runner;
pub mod state;
pub mod status;
#[cfg(target_os = "windows")]
pub mod whisper_local;
