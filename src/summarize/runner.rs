use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};

use crate::config::Config;
use crate::summarize::email::EmailClient;
use crate::summarize::html;
use crate::summarize::llm::LlmClient;
use crate::summarize::prompt;
use crate::transcribe::backend::Transcript;

/// Main entry point for the summarize command.
pub fn run_summarize(config: &Config, range: &str) -> Result<()> {
    let (dates, label, file_suffix) = resolve_date_range(range)?;

    tracing::info!(
        "Summarizing {} ({} date(s): {})",
        file_suffix,
        dates.len(),
        label
    );

    // 1. Load transcripts for the target dates
    let recordings_dir = &config.output.directory;
    let transcripts = load_transcripts(recordings_dir, &dates)?;

    // 2. Check if there are any meaningful transcripts
    let meaningful_count = transcripts
        .iter()
        .filter(|t| !prompt::is_noise(&t.text))
        .count();

    if meaningful_count == 0 {
        tracing::info!("No meaningful transcripts found for {}", label);
        let no_content_msg = format!("No transcripts recorded for {}.", label);
        save_summary(recordings_dir, &file_suffix, &no_content_msg)?;

        // Try to send a short notification email
        match EmailClient::from_config(&config.summarization) {
            Ok(email_client) => {
                let subject = format!("deskmic {} — {}", file_suffix, label);
                let html_body = html::markdown_to_html_email(
                    &no_content_msg,
                    &subject,
                    &label,
                );
                match email_client.send_email(&subject, &no_content_msg, Some(&html_body)) {
                    Ok(_) => tracing::info!("Notification email sent"),
                    Err(e) => tracing::warn!("Failed to send notification email: {:#}", e),
                }
            }
            Err(e) => tracing::info!("Email not configured, skipping: {:#}", e),
        }
        return Ok(());
    }

    tracing::info!(
        "Loaded {} transcripts ({} meaningful) for {}",
        transcripts.len(),
        meaningful_count,
        label
    );

    // 3. Build prompt and call LLM
    let llm = LlmClient::from_config(config)
        .context("Failed to initialize LLM client")?;

    let custom_prompt = &config.summarization.system_prompt;
    let summary = generate_summary(&llm, &label, &transcripts, custom_prompt)?;

    // 4. Save summary locally (always, even if email fails)
    save_summary(recordings_dir, &file_suffix, &summary)?;

    // 5. Send email
    match EmailClient::from_config(&config.summarization) {
        Ok(email_client) => {
            let subject = format!("deskmic {} — {}", file_suffix, label);
            let html_body = html::markdown_to_html_email(&summary, &subject, &label);
            match email_client.send_email(&subject, &summary, Some(&html_body)) {
                Ok(op_id) => {
                    tracing::info!("Summary email sent (operation: {})", op_id);
                }
                Err(e) => {
                    tracing::error!("Failed to send summary email: {:#}", e);
                    tracing::info!("Summary saved locally — check recordings/summaries/");
                }
            }
        }
        Err(e) => {
            tracing::warn!("Email not configured, skipping: {:#}", e);
            tracing::info!("Summary saved locally — check recordings/summaries/");
        }
    }

    println!("Summary generated for {}", label);
    Ok(())
}

/// Parse a date range argument into target dates, a human-readable label, and a file suffix.
///
/// Accepted formats:
/// - `"daily"` → yesterday
/// - `"weekly"` → last 7 days
/// - `"YYYY-MM-DD"` → that specific date
/// - `"YYYY-MM-DD..YYYY-MM-DD"` → inclusive date range (max 90 days)
pub fn resolve_date_range(arg: &str) -> Result<(Vec<NaiveDate>, String, String)> {
    let today = Local::now().date_naive();

    match arg {
        "daily" => {
            let yesterday = today - chrono::Duration::days(1);
            let label = yesterday.format("%Y-%m-%d").to_string();
            let suffix = format!("{}-daily", label);
            Ok((vec![yesterday], label, suffix))
        }
        "weekly" => {
            let mut dates = Vec::new();
            for i in 1..=7 {
                dates.push(today - chrono::Duration::days(i));
            }
            dates.sort();
            let first = dates.first().unwrap().format("%Y-%m-%d").to_string();
            let last = dates.last().unwrap().format("%Y-%m-%d").to_string();
            let label = format!("{} to {}", first, last);
            let suffix = format!("{}-weekly", last);
            Ok((dates, label, suffix))
        }
        _ if arg.contains("..") => {
            let parts: Vec<&str> = arg.splitn(2, "..").collect();
            if parts.len() != 2 {
                anyhow::bail!(
                    "Invalid date range '{}'. Expected YYYY-MM-DD..YYYY-MM-DD",
                    arg
                );
            }
            let start = NaiveDate::parse_from_str(parts[0], "%Y-%m-%d")
                .with_context(|| format!("Invalid start date '{}'", parts[0]))?;
            let end = NaiveDate::parse_from_str(parts[1], "%Y-%m-%d")
                .with_context(|| format!("Invalid end date '{}'", parts[1]))?;

            if end < start {
                anyhow::bail!("End date {} is before start date {}", end, start);
            }

            let day_count = (end - start).num_days() + 1;
            if day_count > 90 {
                anyhow::bail!(
                    "Date range spans {} days (max 90). Use a shorter range.",
                    day_count
                );
            }

            let mut dates = Vec::new();
            let mut d = start;
            while d <= end {
                dates.push(d);
                d += chrono::Duration::days(1);
            }

            let label = format!(
                "{} to {}",
                start.format("%Y-%m-%d"),
                end.format("%Y-%m-%d")
            );
            let suffix = format!(
                "{}-to-{}",
                start.format("%Y-%m-%d"),
                end.format("%Y-%m-%d")
            );
            Ok((dates, label, suffix))
        }
        _ => {
            // Single date
            let date = NaiveDate::parse_from_str(arg, "%Y-%m-%d").with_context(|| {
                format!(
                    "Invalid date range '{}'. Expected: daily, weekly, YYYY-MM-DD, or YYYY-MM-DD..YYYY-MM-DD",
                    arg
                )
            })?;
            let label = date.format("%Y-%m-%d").to_string();
            let suffix = format!("{}-daily", label);
            Ok((vec![date], label, suffix))
        }
    }
}

/// Load JSONL transcripts for the given dates.
fn load_transcripts(recordings_dir: &Path, dates: &[NaiveDate]) -> Result<Vec<Transcript>> {
    let transcript_dir = recordings_dir.join("transcripts");
    let mut all_transcripts = Vec::new();

    for date in dates {
        let filename = format!("{}.jsonl", date.format("%Y-%m-%d"));
        let path = transcript_dir.join(&filename);

        if !path.exists() {
            tracing::debug!("No transcript file for {}", date);
            continue;
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Transcript>(trimmed) {
                Ok(t) => all_transcripts.push(t),
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse line {} of {}: {}",
                        line_num + 1,
                        filename,
                        e
                    );
                }
            }
        }
    }

    Ok(all_transcripts)
}

/// Generate a summary using the LLM, handling chunking if needed.
fn generate_summary(
    llm: &LlmClient,
    date_label: &str,
    transcripts: &[Transcript],
    custom_system_prompt: &str,
) -> Result<String> {
    // Estimate total tokens in transcript content
    let total_text: String = transcripts
        .iter()
        .filter(|t| !prompt::is_noise(&t.text))
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    let estimated_tokens = prompt::estimate_tokens(&total_text);
    tracing::info!("Estimated transcript tokens: {}", estimated_tokens);

    // If content fits in a single pass (~60k token context, leave room for prompt + response)
    const MAX_SINGLE_PASS_TOKENS: usize = 50_000;

    if estimated_tokens <= MAX_SINGLE_PASS_TOKENS {
        // Single pass
        let (system, user) = prompt::build_prompt(date_label, transcripts, custom_system_prompt);
        let summary = llm
            .chat(&system, &user)
            .context("LLM summarization failed")?;
        return Ok(summary);
    }

    // Multi-pass: chunk transcripts, summarize each, then combine
    tracing::info!(
        "Transcript too large for single pass ({}), chunking...",
        estimated_tokens
    );

    let chunks = prompt::chunk_transcripts(transcripts, MAX_SINGLE_PASS_TOKENS / 2);
    let mut partial_summaries = Vec::new();

    for (i, chunk) in chunks.iter().enumerate() {
        tracing::info!("Summarizing chunk {}/{}", i + 1, chunks.len());
        let chunk_label = format!("{} (part {}/{})", date_label, i + 1, chunks.len());
        let (system, user) = prompt::build_prompt(&chunk_label, chunk, custom_system_prompt);
        let partial = llm
            .chat(&system, &user)
            .with_context(|| format!("LLM summarization failed for chunk {}", i + 1))?;
        partial_summaries.push(partial);
    }

    // Combine partial summaries
    let combine_system = format!(
        "You are a personal productivity assistant. Below are partial summaries of voice \
         transcripts from {}. Combine them into a single coherent summary with:\n\
         1. An **Executive Summary** (3-5 bullet points)\n\
         2. A **Detailed Breakdown** organized by hour\n\n\
         Deduplicate and merge overlapping content. Keep it concise.",
        date_label
    );

    let combine_user = partial_summaries
        .iter()
        .enumerate()
        .map(|(i, s)| format!("## Partial Summary {}\n\n{}", i + 1, s))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    let final_summary = llm
        .chat(&combine_system, &combine_user)
        .context("LLM combination pass failed")?;

    Ok(final_summary)
}

/// Save the summary to a local markdown file.
fn save_summary(recordings_dir: &Path, file_suffix: &str, content: &str) -> Result<PathBuf> {
    let summary_dir = recordings_dir.join("summaries");
    std::fs::create_dir_all(&summary_dir)
        .with_context(|| format!("Failed to create {}", summary_dir.display()))?;

    let path = summary_dir.join(format!("{}.md", file_suffix));
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    tracing::info!("Summary saved to {}", path.display());
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_date_range_daily() {
        let (dates, label, suffix) = resolve_date_range("daily").unwrap();
        assert_eq!(dates.len(), 1);
        assert!(label.len() == 10); // YYYY-MM-DD
        assert!(suffix.ends_with("-daily"));
    }

    #[test]
    fn test_resolve_date_range_weekly() {
        let (dates, label, suffix) = resolve_date_range("weekly").unwrap();
        assert_eq!(dates.len(), 7);
        assert!(label.contains(" to "));
        assert!(suffix.ends_with("-weekly"));
        // Dates should be sorted ascending
        for i in 1..dates.len() {
            assert!(dates[i] > dates[i - 1]);
        }
    }

    #[test]
    fn test_resolve_date_range_specific_date() {
        let (dates, label, suffix) = resolve_date_range("2026-02-15").unwrap();
        assert_eq!(dates.len(), 1);
        assert_eq!(dates[0], NaiveDate::from_ymd_opt(2026, 2, 15).unwrap());
        assert_eq!(label, "2026-02-15");
        assert_eq!(suffix, "2026-02-15-daily");
    }

    #[test]
    fn test_resolve_date_range_range() {
        let (dates, label, suffix) = resolve_date_range("2026-02-10..2026-02-14").unwrap();
        assert_eq!(dates.len(), 5);
        assert_eq!(dates[0], NaiveDate::from_ymd_opt(2026, 2, 10).unwrap());
        assert_eq!(dates[4], NaiveDate::from_ymd_opt(2026, 2, 14).unwrap());
        assert_eq!(label, "2026-02-10 to 2026-02-14");
        assert_eq!(suffix, "2026-02-10-to-2026-02-14");
    }

    #[test]
    fn test_resolve_date_range_single_day_range() {
        let (dates, _label, _suffix) = resolve_date_range("2026-02-15..2026-02-15").unwrap();
        assert_eq!(dates.len(), 1);
    }

    #[test]
    fn test_resolve_date_range_invalid() {
        assert!(resolve_date_range("garbage").is_err());
        assert!(resolve_date_range("2026-02-15..2026-02-10").is_err()); // end before start
    }

    #[test]
    fn test_resolve_date_range_too_long() {
        assert!(resolve_date_range("2025-01-01..2025-12-31").is_err()); // 365 days > 90
    }

    #[test]
    fn test_load_transcripts_missing_dir() {
        let result = load_transcripts(
            Path::new("/nonexistent"),
            &[NaiveDate::from_ymd_opt(2026, 2, 17).unwrap()],
        );
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_load_transcripts_from_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let transcript_dir = tmp.path().join("transcripts");
        std::fs::create_dir_all(&transcript_dir).unwrap();

        let jsonl = r#"{"timestamp":"2026-02-17","source":"mic","duration_secs":8.0,"file":"mic_14-30-00.wav","text":"Hello"}
{"timestamp":"2026-02-17","source":"mic","duration_secs":8.0,"file":"mic_15-00-00.wav","text":"World"}"#;
        std::fs::write(transcript_dir.join("2026-02-17.jsonl"), jsonl).unwrap();

        let dates = vec![NaiveDate::from_ymd_opt(2026, 2, 17).unwrap()];
        let transcripts = load_transcripts(tmp.path(), &dates).unwrap();
        assert_eq!(transcripts.len(), 2);
        assert_eq!(transcripts[0].text, "Hello");
        assert_eq!(transcripts[1].text, "World");
    }

    #[test]
    fn test_save_summary() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = save_summary(tmp.path(), "2026-02-17-daily", "# Test Summary").unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# Test Summary");
    }
}
