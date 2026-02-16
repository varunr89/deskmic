#[cfg(target_os = "windows")]
pub mod capture;
pub mod pipeline;
pub mod ring_buffer;
#[cfg(target_os = "windows")]
pub mod teams_capture;
pub mod teams_monitor;
pub mod vad;
