use std::collections::BTreeMap;

use crate::transcribe::backend::Transcript;

/// Noise patterns that should be filtered from transcripts before summarization.
const NOISE_PATTERNS: &[&str] = &[
    "[BLANK_AUDIO]",
    "[blank_audio]",
    "(keyboard clicking)",
    "(keyboard clacking)",
    "[snoring]",
    "(coughing)",
    "(silence)",
    "[silence]",
    "(music)",
    "[music]",
    "(static)",
    "(background noise)",
];

/// Returns true if the transcript text is considered noise (empty, whitespace-only,
/// or matches known noise patterns).
pub fn is_noise(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    NOISE_PATTERNS
        .iter()
        .any(|p| trimmed.eq_ignore_ascii_case(p))
}

/// Extract the hour from a filename like "mic_14-30-00.wav" → 14.
/// Returns None if the filename doesn't match the expected pattern.
pub fn extract_hour(filename: &str) -> Option<u32> {
    // Strip the source prefix (e.g. "mic_" or "teams_")
    let time_part = filename.split('_').nth(1)?;
    // Parse "HH-MM-SS.wav" → HH
    let hour_str = time_part.split('-').next()?;
    hour_str.parse().ok()
}

/// Group transcripts by hour based on filename timestamps.
/// Returns a BTreeMap so hours are in sorted order.
pub fn group_by_hour<'a>(transcripts: &[&'a Transcript]) -> BTreeMap<u32, Vec<&'a Transcript>> {
    let mut groups: BTreeMap<u32, Vec<&'a Transcript>> = BTreeMap::new();
    for t in transcripts {
        if let Some(hour) = extract_hour(&t.file) {
            groups.entry(hour).or_default().push(t);
        }
    }
    groups
}

/// Format a single hour's transcripts into a readable block for the LLM prompt.
fn format_hour_block(hour: u32, transcripts: &[&Transcript]) -> String {
    let mut lines = Vec::new();
    let hour_label = format!("{:02}:00–{:02}:59", hour, hour);
    lines.push(format!("### {}", hour_label));
    lines.push(String::new());

    for t in transcripts {
        // Include the source and time from filename for context
        let time_tag = t
            .file
            .split('_')
            .nth(1)
            .unwrap_or("")
            .trim_end_matches(".wav")
            .replace('-', ":");
        let source_tag = if t.source == "mic" { "Mic" } else { "App" };
        lines.push(format!("[{} {}] {}", time_tag, source_tag, t.text.trim()));
    }

    lines.push(String::new());
    lines.join("\n")
}

/// Build the full prompt for a single summarization pass.
/// `date_label` is something like "2026-02-17" or "2026-02-11 to 2026-02-17".
pub fn build_prompt(
    date_label: &str,
    transcripts: &[Transcript],
) -> (String, String) {
    let system = format!(
        "You are a personal productivity assistant. You will be given raw voice transcripts \
         from a desk microphone recording of a workday ({date_label}). The transcripts contain \
         meetings, conversations, and ambient audio captured throughout the day.\n\n\
         Your task:\n\
         1. Write an **Executive Summary** (3-5 bullet points) of the most important topics, \
            decisions, and action items from the day.\n\
         2. Write a **Detailed Breakdown** organized by hour, summarizing what was discussed \
            in each time block. Omit hours with no meaningful content.\n\n\
         Guidelines:\n\
         - Focus on actionable information: decisions made, tasks assigned, key discussion points.\n\
         - Ignore background noise, small talk, and filler.\n\
         - Use clear, professional language.\n\
         - Format the output in Markdown.\n\
         - If you recognize names, use them. Otherwise, use generic labels like \"Speaker A\".\n\
         - Keep the summary concise but complete. Target 200-500 words for a daily summary.",
        date_label = date_label,
    );

    let filtered: Vec<&Transcript> = transcripts
        .iter()
        .filter(|t| !is_noise(&t.text))
        .collect();

    let grouped = group_by_hour(&filtered);

    let mut user_parts = Vec::new();
    user_parts.push(format!(
        "# Transcripts for {}\n\nTotal segments: {} (after noise filtering)\n",
        date_label,
        filtered.len()
    ));

    for (hour, hour_transcripts) in &grouped {
        user_parts.push(format_hour_block(*hour, hour_transcripts));
    }

    (system, user_parts.join("\n"))
}

/// Estimate token count from text (rough: ~4 chars per token for English).
pub fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

/// If the transcript text is too large for a single pass, chunk it by hour groups
/// that each fit within `max_tokens_per_chunk`.
pub fn chunk_transcripts(
    transcripts: &[Transcript],
    max_tokens_per_chunk: usize,
) -> Vec<Vec<Transcript>> {
    let filtered: Vec<&Transcript> = transcripts
        .iter()
        .filter(|t| !is_noise(&t.text))
        .collect();

    let grouped = group_by_hour(&filtered);
    let mut chunks: Vec<Vec<Transcript>> = Vec::new();
    let mut current_chunk: Vec<Transcript> = Vec::new();
    let mut current_tokens: usize = 0;

    for (_hour, hour_transcripts) in &grouped {
        let hour_text: String = hour_transcripts
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let hour_tokens = estimate_tokens(&hour_text);

        if !current_chunk.is_empty() && current_tokens + hour_tokens > max_tokens_per_chunk {
            chunks.push(std::mem::take(&mut current_chunk));
            current_tokens = 0;
        }

        for t in hour_transcripts {
            current_chunk.push((*t).clone());
        }
        current_tokens += hour_tokens;
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    // If everything fit in one chunk (or was empty), return as-is
    if chunks.is_empty() {
        chunks.push(Vec::new());
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_transcript(file: &str, text: &str) -> Transcript {
        Transcript {
            timestamp: "2026-02-17".to_string(),
            source: "mic".to_string(),
            duration_secs: 8.0,
            file: file.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn test_is_noise_empty() {
        assert!(is_noise(""));
        assert!(is_noise("   "));
    }

    #[test]
    fn test_is_noise_patterns() {
        assert!(is_noise("[BLANK_AUDIO]"));
        assert!(is_noise("(keyboard clicking)"));
        assert!(is_noise("[snoring]"));
        assert!(is_noise("(coughing)"));
    }

    #[test]
    fn test_is_noise_real_speech() {
        assert!(!is_noise("Hello, how are you?"));
        assert!(!is_noise("Test, test, test."));
    }

    #[test]
    fn test_extract_hour() {
        assert_eq!(extract_hour("mic_14-30-00.wav"), Some(14));
        assert_eq!(extract_hour("teams_09-15-30.wav"), Some(9));
        assert_eq!(extract_hour("mic_00-00-00.wav"), Some(0));
        assert_eq!(extract_hour("mic_23-59-59.wav"), Some(23));
        assert_eq!(extract_hour("invalid.wav"), None);
    }

    #[test]
    fn test_group_by_hour() {
        let transcripts = vec![
            make_transcript("mic_14-30-00.wav", "Hello"),
            make_transcript("mic_14-45-00.wav", "World"),
            make_transcript("mic_15-00-00.wav", "Goodbye"),
        ];
        let refs: Vec<&Transcript> = transcripts.iter().collect();
        let groups = group_by_hour(&refs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[&14].len(), 2);
        assert_eq!(groups[&15].len(), 1);
    }

    #[test]
    fn test_build_prompt_filters_noise() {
        let transcripts = vec![
            make_transcript("mic_14-30-00.wav", "Important meeting topic"),
            make_transcript("mic_14-35-00.wav", ""),
            make_transcript("mic_14-40-00.wav", "[BLANK_AUDIO]"),
            make_transcript("mic_15-00-00.wav", "Action item discussed"),
        ];
        let (_system, user) = build_prompt("2026-02-17", &transcripts);
        assert!(user.contains("Important meeting topic"));
        assert!(user.contains("Action item discussed"));
        assert!(!user.contains("[BLANK_AUDIO]"));
        assert!(user.contains("Total segments: 2"));
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("a]b"), 0); // 3 chars / 4 = 0
        let long = "a".repeat(400);
        assert_eq!(estimate_tokens(&long), 100);
    }

    #[test]
    fn test_chunk_transcripts_single_chunk() {
        let transcripts = vec![
            make_transcript("mic_14-30-00.wav", "Hello world"),
        ];
        let chunks = chunk_transcripts(&transcripts, 100_000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 1);
    }

    #[test]
    fn test_chunk_transcripts_splits_on_size() {
        let long_text = "word ".repeat(2000); // ~10000 chars = ~2500 tokens
        let transcripts = vec![
            make_transcript("mic_10-00-00.wav", &long_text),
            make_transcript("mic_11-00-00.wav", &long_text),
            make_transcript("mic_12-00-00.wav", &long_text),
        ];
        // Each chunk ~2500 tokens, limit to 3000 → should split into 3 chunks
        let chunks = chunk_transcripts(&transcripts, 3000);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn test_chunk_transcripts_empty() {
        let transcripts: Vec<Transcript> = vec![];
        let chunks = chunk_transcripts(&transcripts, 100_000);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].is_empty());
    }
}
