use anyhow::Result;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Validation helpers (standalone fns so they are unit-testable)
// ---------------------------------------------------------------------------

fn validate_url(s: &str) -> bool {
    !s.is_empty() && s.starts_with("https://")
}

fn validate_email(s: &str) -> bool {
    !s.is_empty() && s.contains('@')
}

// ---------------------------------------------------------------------------
// Input helpers
// ---------------------------------------------------------------------------

/// Display numbered options, return the 0-indexed choice.
fn prompt_choice(question: &str, options: &[&str], default: usize) -> usize {
    println!("  {question}");
    for (i, opt) in options.iter().enumerate() {
        let marker = if i == default { " (default)" } else { "" };
        println!("    {}. {}{}", i + 1, opt, marker);
    }
    print!("  Choice [{}]: ", default + 1);
    let _ = io::stdout().flush();

    let mut buf = String::new();
    if io::stdin().read_line(&mut buf).is_err() {
        return default;
    }
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        return default;
    }
    match trimmed.parse::<usize>() {
        Ok(n) if n >= 1 && n <= options.len() => n - 1,
        _ => default,
    }
}

/// Yes/no prompt. Returns `default` when the user presses Enter.
fn prompt_yn(question: &str, default: bool) -> bool {
    let hint = if default { "Y/n" } else { "y/N" };
    print!("  {} ({hint}): ", question);
    let _ = io::stdout().flush();

    let mut buf = String::new();
    if io::stdin().read_line(&mut buf).is_err() {
        return default;
    }
    let trimmed = buf.trim().to_lowercase();
    if trimmed.is_empty() {
        return default;
    }
    matches!(trimmed.as_str(), "y" | "yes")
}

/// Read a single trimmed line from stdin.
fn prompt_input(label: &str) -> String {
    print!("  {label}: ");
    let _ = io::stdout().flush();

    let mut buf = String::new();
    let _ = io::stdin().read_line(&mut buf);
    buf.trim().to_string()
}

/// Keep prompting until `validator` returns `true`.
fn prompt_validated(label: &str, validator: fn(&str) -> bool, error_msg: &str) -> String {
    loop {
        let value = prompt_input(label);
        if validator(&value) {
            return value;
        }
        println!("  {error_msg}");
    }
}

fn nonempty(s: &str) -> bool {
    !s.trim().is_empty()
}

// ---------------------------------------------------------------------------
// Exe-relative path helpers
// ---------------------------------------------------------------------------

fn exe_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Could not determine executable directory"))
}

fn config_path() -> Result<PathBuf> {
    Ok(exe_dir()?.join("deskmic.toml"))
}

fn model_path(name: &str) -> Result<PathBuf> {
    Ok(exe_dir()?.join(format!("ggml-{name}.bin")))
}

// ---------------------------------------------------------------------------
// Config file updater – operates on the raw TOML text produced by
// Config::generate_default_commented() so we can uncomment and fill values.
// ---------------------------------------------------------------------------

struct SummarizationCredentials {
    azure_endpoint: String,
    azure_api_key: String,
    chat_deployment: String,
    acs_endpoint: String,
    acs_api_key: String,
    sender_address: String,
    recipient_address: String,
}

fn update_config_with_summarization(
    config_text: &str,
    creds: &SummarizationCredentials,
) -> String {
    // Map of commented-out key → new value to substitute.
    // Each entry is (key_name, replacement_value).
    let replacements: &[(&str, &str)] = &[
        ("endpoint", &creds.azure_endpoint),
        ("api_key", &creds.azure_api_key),
        ("deployment", &creds.chat_deployment),
        ("acs_endpoint", &creds.acs_endpoint),
        ("acs_api_key", &creds.acs_api_key),
        ("sender_address", &creds.sender_address),
        ("recipient_address", &creds.recipient_address),
    ];

    let mut lines: Vec<String> = config_text.lines().map(String::from).collect();

    for (key, value) in replacements {
        for line in lines.iter_mut() {
            let trimmed = line.trim();
            // Match lines like:  # key = "..."
            // We look for the commented-out pattern: starts with '#' and contains `key =`
            if trimmed.starts_with('#') {
                // Strip the leading '#' and optional space to get the inner content.
                let inner = trimmed.trim_start_matches('#').trim();
                // Check if this inner content starts with `key = ` or `key ="`
                if inner.starts_with(&format!("{key} =")) {
                    *line = format!("{key} = \"{value}\"");
                }
            }
        }
    }

    let mut result = lines.join("\n");
    // Preserve trailing newline if the original had one.
    if config_text.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

// ---------------------------------------------------------------------------
// Step 1 – Download Whisper model
// ---------------------------------------------------------------------------

const HF_BASE_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

const MODEL_OPTIONS: &[(&str, &str, &str)] = &[
    ("tiny.en", "ggml-tiny.en.bin", "~75 MB"),
    ("base.en", "ggml-base.en.bin", "~142 MB"),
    ("small.en", "ggml-small.en.bin", "~466 MB"),
];

fn step_download_model() {
    println!();
    println!("  [1/4] Whisper Model");
    println!("  -------------------");
    println!();

    let labels: Vec<String> = MODEL_OPTIONS
        .iter()
        .map(|(name, _, size)| format!("{name} ({size})"))
        .collect();
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let choice = prompt_choice("Which Whisper model?", &label_refs, 1);

    let (name, filename, _) = MODEL_OPTIONS[choice];

    let dest = match model_path(name) {
        Ok(p) => p,
        Err(e) => {
            println!("  Warning: could not determine model path: {e}");
            return;
        }
    };

    // If already present, ask before re-downloading.
    if dest.exists() {
        if !prompt_yn(
            &format!("  {filename} already exists. Re-download?"),
            false,
        ) {
            println!("  Skipping download.");
            return;
        }
    }

    let url = format!("{HF_BASE_URL}{filename}");
    println!("  Downloading {filename} from Hugging Face...");

    if let Err(e) = download_file(&url, &dest) {
        println!("  Warning: download failed: {e}");
    } else {
        println!("  Saved to {}", dest.display());
    }
}

fn download_file(url: &str, dest: &Path) -> Result<()> {
    let part_path = dest.with_extension("bin.part");

    let response = reqwest::blocking::get(url)?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("HTTP {status}");
    }

    let total_bytes = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let mut file = std::fs::File::create(&part_path)?;
    let mut downloaded: u64 = 0;
    let mut reader = response;
    let mut buf = [0u8; 8192];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;

        // Progress line using \r.
        if let Some(total) = total_bytes {
            let pct = (downloaded as f64 / total as f64 * 100.0) as u32;
            let mb_done = downloaded as f64 / 1_048_576.0;
            let mb_total = total as f64 / 1_048_576.0;
            print!(
                "\r  [{pct:>3}%] {mb_done:.1} / {mb_total:.1} MB",
            );
        } else {
            let mb_done = downloaded as f64 / 1_048_576.0;
            print!("\r  {mb_done:.1} MB downloaded");
        }
        let _ = io::stdout().flush();
    }
    println!(); // finish the progress line

    drop(file);
    std::fs::rename(&part_path, dest)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 2 – Generate default config
// ---------------------------------------------------------------------------

fn step_generate_config() {
    println!();
    println!("  [2/4] Configuration File");
    println!("  ------------------------");
    println!();

    let path = match config_path() {
        Ok(p) => p,
        Err(e) => {
            println!("  Warning: could not determine config path: {e}");
            return;
        }
    };

    if path.exists() {
        if !prompt_yn(
            &format!("{} already exists. Overwrite?", path.display()),
            false,
        ) {
            println!("  Skipping config generation.");
            return;
        }
    }

    let content = crate::config::Config::generate_default_commented();
    match std::fs::write(&path, &content) {
        Ok(_) => println!("  Wrote {}", path.display()),
        Err(e) => println!("  Warning: could not write config: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Step 3 – Summarization setup (optional)
// ---------------------------------------------------------------------------

fn step_summarization() {
    println!();
    println!("  [3/4] Email Summaries (optional)");
    println!("  --------------------------------");
    println!();

    let choice = prompt_choice(
        "Would you like daily email summaries of your transcripts?",
        &["Yes", "No"],
        1,
    );
    if choice != 0 {
        println!("  Skipping summarization setup.");
        return;
    }

    println!();

    let creds = SummarizationCredentials {
        azure_endpoint: prompt_validated(
            "Azure OpenAI endpoint",
            validate_url,
            "Must start with https://",
        ),
        azure_api_key: prompt_validated(
            "Azure OpenAI API key",
            nonempty,
            "API key cannot be empty",
        ),
        chat_deployment: prompt_validated(
            "Chat deployment name",
            nonempty,
            "Deployment name cannot be empty",
        ),
        acs_endpoint: prompt_validated(
            "ACS endpoint",
            validate_url,
            "Must start with https://",
        ),
        acs_api_key: prompt_validated(
            "ACS API key",
            nonempty,
            "API key cannot be empty",
        ),
        sender_address: prompt_validated(
            "Sender email address",
            validate_email,
            "Must be a valid email address (contains @)",
        ),
        recipient_address: prompt_validated(
            "Recipient email address",
            validate_email,
            "Must be a valid email address (contains @)",
        ),
    };

    // Update config file with the collected credentials.
    let path = match config_path() {
        Ok(p) => p,
        Err(e) => {
            println!("  Warning: could not determine config path: {e}");
            return;
        }
    };

    if !path.exists() {
        println!("  Warning: config file not found at {}. Run step 2 first.", path.display());
        return;
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let updated = update_config_with_summarization(&content, &creds);
            match std::fs::write(&path, &updated) {
                Ok(_) => println!("  Updated {}", path.display()),
                Err(e) => println!("  Warning: could not write config: {e}"),
            }
        }
        Err(e) => {
            println!("  Warning: could not read config: {e}");
            return;
        }
    }

    // Create scheduled tasks (Windows only).
    create_scheduled_tasks();
}

#[cfg(target_os = "windows")]
fn create_scheduled_tasks() {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            println!("  Warning: could not determine exe path: {e}");
            return;
        }
    };
    let exe_str = exe.display().to_string();

    let tasks = [
        (
            "deskmic-daily-summary",
            format!("\"{}\" summarize --period daily", exe_str),
            vec!["/SC", "DAILY", "/ST", "07:00"],
        ),
        (
            "deskmic-weekly-summary",
            format!("\"{}\" summarize --period weekly", exe_str),
            vec!["/SC", "WEEKLY", "/D", "MON", "/ST", "07:00"],
        ),
    ];

    for (name, tr, extra) in &tasks {
        let mut cmd = std::process::Command::new("schtasks");
        cmd.args(["/Create", "/TN", name, "/TR", tr.as_str()]);
        cmd.args(extra);
        cmd.arg("/F");

        match cmd.output() {
            Ok(output) if output.status.success() => {
                println!("  Created scheduled task: {name}");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("  Warning: schtasks failed for {name}: {stderr}");
            }
            Err(e) => {
                println!("  Warning: could not run schtasks for {name}: {e}");
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn create_scheduled_tasks() {
    println!("  Scheduled tasks are only supported on Windows.");
}

// ---------------------------------------------------------------------------
// Step 4 – Auto-start (optional)
// ---------------------------------------------------------------------------

fn step_autostart() {
    println!();
    println!("  [4/4] Windows Startup");
    println!("  ---------------------");
    println!();

    if !prompt_yn("Add deskmic to Windows startup?", true) {
        println!("  Skipping startup installation.");
        return;
    }

    install_startup_wrapper();
}

#[cfg(target_os = "windows")]
fn install_startup_wrapper() {
    match crate::commands::install_startup() {
        Ok(_) => println!("  Added to Windows startup."),
        Err(e) => println!("  Warning: could not add to startup: {e}"),
    }
}

#[cfg(not(target_os = "windows"))]
fn install_startup_wrapper() {
    println!("  Startup installation is only supported on Windows.");
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn run_setup() -> Result<()> {
    println!();
    println!("  deskmic setup");
    println!("  =============");

    step_download_model();
    step_generate_config();
    step_summarization();
    step_autostart();

    println!();
    println!("  Setup complete!");
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- validate_url -------------------------------------------------------

    #[test]
    fn test_validate_url() {
        assert!(validate_url("https://foo.com"));
        assert!(!validate_url("http://foo.com"));
        assert!(!validate_url(""));
    }

    // -- validate_email -----------------------------------------------------

    #[test]
    fn test_validate_email() {
        assert!(validate_email("a@b.com"));
        assert!(!validate_email("nope"));
        assert!(!validate_email(""));
    }

    // -- config update ------------------------------------------------------

    #[test]
    fn test_config_update_uncomments_summarization() {
        let sample = r#"[transcription.azure]
# endpoint = "https://your-resource.openai.azure.com"
# api_key = ""

[summarization]
# deployment = "gpt-4o"
# acs_endpoint = "https://your-acs.unitedstates.communication.azure.com"
# acs_api_key = ""
# sender_address = "DoNotReply@your-domain.azurecomm.net"
# recipient_address = "you@example.com"
"#;

        let creds = SummarizationCredentials {
            azure_endpoint: "https://my-oai.openai.azure.com".into(),
            azure_api_key: "oai-secret-123".into(),
            chat_deployment: "gpt-4o-mini".into(),
            acs_endpoint: "https://my-acs.communication.azure.com".into(),
            acs_api_key: "acs-secret-456".into(),
            sender_address: "bot@contoso.azurecomm.net".into(),
            recipient_address: "alice@example.com".into(),
        };

        let updated = update_config_with_summarization(sample, &creds);

        // [transcription.azure] values should be uncommented and filled
        assert!(
            updated.contains(r#"endpoint = "https://my-oai.openai.azure.com""#),
            "expected azure endpoint, got:\n{updated}"
        );
        assert!(
            updated.contains(r#"api_key = "oai-secret-123""#),
            "expected azure api_key, got:\n{updated}"
        );

        // [summarization] values should be uncommented and filled
        assert!(
            updated.contains(r#"deployment = "gpt-4o-mini""#),
            "expected deployment, got:\n{updated}"
        );
        assert!(
            updated.contains(r#"acs_endpoint = "https://my-acs.communication.azure.com""#),
            "expected acs_endpoint, got:\n{updated}"
        );
        assert!(
            updated.contains(r#"acs_api_key = "acs-secret-456""#),
            "expected acs_api_key, got:\n{updated}"
        );
        assert!(
            updated.contains(r#"sender_address = "bot@contoso.azurecomm.net""#),
            "expected sender_address, got:\n{updated}"
        );
        assert!(
            updated.contains(r#"recipient_address = "alice@example.com""#),
            "expected recipient_address, got:\n{updated}"
        );

        // Lines should no longer be commented out
        assert!(
            !updated.contains("# deployment"),
            "deployment should be uncommented"
        );
        assert!(
            !updated.contains("# acs_endpoint"),
            "acs_endpoint should be uncommented"
        );
    }
}
