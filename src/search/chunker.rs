use sha2::{Digest, Sha256};

use crate::transcribe::backend::Transcript;

/// A conversation chunk — a group of temporally-close utterances from the same source.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Deterministic ID: SHA-256 hex of "{date}|{source}|{start_file}".
    pub id: String,
    /// Date string (YYYY-MM-DD) from the transcript timestamp field.
    pub date: String,
    /// Audio source (e.g. "mic", "teams").
    pub source: String,
    /// Start time extracted from the first filename (e.g. "09-37-31").
    pub start_time: String,
    /// End time: start time of last utterance + its duration.
    pub end_time: String,
    /// Total audio duration of all constituent utterances in seconds.
    pub duration_secs: f64,
    /// Concatenated text of all utterances, space-separated.
    pub text: String,
    /// List of constituent WAV filenames.
    pub files: Vec<String>,
}

/// Extract time from filename: "mic_09-37-31.wav" -> "09-37-31"
fn extract_time_from_filename(filename: &str) -> Option<String> {
    // Strip the extension, then take everything after the last '_'
    let stem = filename.strip_suffix(".wav")?;
    let time_part = stem.rsplit('_').next()?;
    // Validate format: HH-MM-SS (8 chars, digits and dashes)
    if time_part.len() != 8 {
        return None;
    }
    let parts: Vec<&str> = time_part.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    for part in &parts {
        if part.len() != 2 || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    Some(time_part.to_string())
}

/// Convert "HH-MM-SS" to seconds since midnight.
fn time_to_secs(time: &str) -> Option<f64> {
    let parts: Vec<&str> = time.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].parse().ok()?;
    let m: f64 = parts[1].parse().ok()?;
    let s: f64 = parts[2].parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

/// Convert seconds since midnight to "HH-MM-SS".
fn secs_to_time(secs: f64) -> String {
    let total = secs.round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{:02}-{:02}-{:02}", h, m, s)
}

/// Deterministic chunk ID: SHA-256 hex of "{date}|{source}|{start_file}".
fn chunk_id(date: &str, source: &str, start_file: &str) -> String {
    let input = format!("{}|{}|{}", date, source, start_file);
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Chunk a list of transcripts into conversation segments.
///
/// Splitting rules:
/// 1. Gap between end of previous utterance and start of next exceeds `chunk_gap_secs`
/// 2. Current chunk exceeds `chunk_max_duration_secs` of total audio duration
/// 3. Source changes (e.g. mic -> teams)
pub fn chunk_transcripts(
    transcripts: &[Transcript],
    chunk_gap_secs: u64,
    chunk_max_duration_secs: u64,
) -> Vec<Chunk> {
    if transcripts.is_empty() {
        return Vec::new();
    }

    // Sort transcripts by filename (lexicographic = chronological).
    let mut sorted: Vec<&Transcript> = transcripts.iter().collect();
    sorted.sort_by(|a, b| a.file.cmp(&b.file));

    let mut chunks: Vec<Chunk> = Vec::new();

    // State for the current chunk being built.
    let mut current_texts: Vec<&str> = Vec::new();
    let mut current_files: Vec<String> = Vec::new();
    let mut current_duration: f64 = 0.0;
    let mut current_source: &str = "";
    let mut current_date: &str = "";
    let mut current_start_file: &str = "";
    let mut current_start_time: String = String::new();
    let mut prev_end_secs: f64 = 0.0;

    for (i, t) in sorted.iter().enumerate() {
        let t_time = extract_time_from_filename(&t.file).unwrap_or_default();
        let t_start_secs = time_to_secs(&t_time).unwrap_or(0.0);

        if i == 0 {
            // First utterance — start the first chunk.
            current_source = &t.source;
            current_date = &t.timestamp;
            current_start_file = &t.file;
            current_start_time = t_time.clone();
            current_texts.push(&t.text);
            current_files.push(t.file.clone());
            current_duration = t.duration_secs;
            prev_end_secs = t_start_secs + t.duration_secs;
            continue;
        }

        let gap = t_start_secs - prev_end_secs;
        let source_changed = t.source != current_source;
        let exceeds_max = current_duration >= chunk_max_duration_secs as f64;

        if source_changed || gap > chunk_gap_secs as f64 || exceeds_max {
            // Finalize the current chunk.
            let end_time = secs_to_time(prev_end_secs);
            chunks.push(Chunk {
                id: chunk_id(current_date, current_source, current_start_file),
                date: current_date.to_string(),
                source: current_source.to_string(),
                start_time: current_start_time.clone(),
                end_time,
                duration_secs: current_duration,
                text: current_texts.join(" "),
                files: current_files.clone(),
            });

            // Start a new chunk.
            current_texts.clear();
            current_files.clear();
            current_source = &t.source;
            current_date = &t.timestamp;
            current_start_file = &t.file;
            current_start_time = t_time.clone();
            current_duration = 0.0;
        }

        current_texts.push(&t.text);
        current_files.push(t.file.clone());
        current_duration += t.duration_secs;
        prev_end_secs = t_start_secs + t.duration_secs;
    }

    // Finalize the last chunk.
    if !current_texts.is_empty() {
        let end_time = secs_to_time(prev_end_secs);
        chunks.push(Chunk {
            id: chunk_id(current_date, current_source, current_start_file),
            date: current_date.to_string(),
            source: current_source.to_string(),
            start_time: current_start_time,
            end_time,
            duration_secs: current_duration,
            text: current_texts.join(" "),
            files: current_files,
        });
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::backend::Transcript;

    fn make_transcript(source: &str, file: &str, text: &str, duration: f64) -> Transcript {
        Transcript {
            timestamp: "2026-03-16".to_string(),
            source: source.to_string(),
            duration_secs: duration,
            file: file.to_string(),
            text: text.to_string(),
        }
    }

    // ── 1. extract_time_from_filename ──────────────────────────────────

    #[test]
    fn test_extract_time_from_filename() {
        // Valid patterns
        assert_eq!(
            extract_time_from_filename("mic_09-37-31.wav"),
            Some("09-37-31".to_string())
        );
        assert_eq!(
            extract_time_from_filename("teams_14-05-00.wav"),
            Some("14-05-00".to_string())
        );
        assert_eq!(
            extract_time_from_filename("some_prefix_23-59-59.wav"),
            Some("23-59-59".to_string())
        );

        // Invalid patterns
        assert_eq!(extract_time_from_filename("mic_09-37.wav"), None); // too short
        assert_eq!(extract_time_from_filename("mic_notime.wav"), None); // not a time
        assert_eq!(extract_time_from_filename("badfile.txt"), None); // wrong extension
        assert_eq!(extract_time_from_filename("mic_09-37-31"), None); // no .wav
    }

    // ── 2. time_to_secs / secs_to_time roundtrips ─────────────────────

    #[test]
    fn test_time_conversions() {
        // time_to_secs
        assert_eq!(time_to_secs("00-00-00"), Some(0.0));
        assert_eq!(time_to_secs("01-00-00"), Some(3600.0));
        assert_eq!(
            time_to_secs("09-37-31"),
            Some(9.0 * 3600.0 + 37.0 * 60.0 + 31.0)
        );
        assert_eq!(time_to_secs("23-59-59"), Some(86399.0));

        // secs_to_time
        assert_eq!(secs_to_time(0.0), "00-00-00");
        assert_eq!(secs_to_time(3600.0), "01-00-00");
        assert_eq!(secs_to_time(86399.0), "23-59-59");

        // Roundtrip
        let original = "14-30-45";
        let secs = time_to_secs(original).unwrap();
        assert_eq!(secs_to_time(secs), original);

        // Invalid input
        assert_eq!(time_to_secs("invalid"), None);
        assert_eq!(time_to_secs("12:30:00"), None);
    }

    // ── 3. chunk_id determinism ───────────────────────────────────────

    #[test]
    fn test_chunk_id_is_deterministic() {
        let id1 = chunk_id("2026-03-16", "mic", "mic_09-37-31.wav");
        let id2 = chunk_id("2026-03-16", "mic", "mic_09-37-31.wav");
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 64); // SHA-256 hex = 64 chars
        assert!(id1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── 4. chunk_id differs for different inputs ──────────────────────

    #[test]
    fn test_chunk_id_differs_for_different_inputs() {
        let id_a = chunk_id("2026-03-16", "mic", "mic_09-37-31.wav");
        let id_b = chunk_id("2026-03-16", "teams", "mic_09-37-31.wav");
        let id_c = chunk_id("2026-03-17", "mic", "mic_09-37-31.wav");
        let id_d = chunk_id("2026-03-16", "mic", "mic_10-00-00.wav");
        assert_ne!(id_a, id_b);
        assert_ne!(id_a, id_c);
        assert_ne!(id_a, id_d);
    }

    // ── 5. empty transcripts ──────────────────────────────────────────

    #[test]
    fn test_empty_transcripts_returns_empty() {
        let chunks = chunk_transcripts(&[], 300, 300);
        assert!(chunks.is_empty());
    }

    // ── 6. single utterance ───────────────────────────────────────────

    #[test]
    fn test_single_utterance_becomes_one_chunk() {
        let transcripts = vec![make_transcript(
            "mic",
            "mic_09-37-31.wav",
            "hello world",
            10.0,
        )];
        let chunks = chunk_transcripts(&transcripts, 300, 300);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "hello world");
        assert_eq!(chunks[0].source, "mic");
        assert_eq!(chunks[0].date, "2026-03-16");
        assert_eq!(chunks[0].start_time, "09-37-31");
        assert_eq!(chunks[0].files, vec!["mic_09-37-31.wav"]);
        assert!((chunks[0].duration_secs - 10.0).abs() < 0.01);
    }

    // ── 7. close utterances grouped together ──────────────────────────

    #[test]
    fn test_close_utterances_grouped_together() {
        // Three utterances within 10s of each other — well within default 300s gap.
        let transcripts = vec![
            make_transcript("mic", "mic_09-00-00.wav", "first", 5.0),
            make_transcript("mic", "mic_09-00-10.wav", "second", 5.0),
            make_transcript("mic", "mic_09-00-20.wav", "third", 5.0),
        ];
        let chunks = chunk_transcripts(&transcripts, 300, 300);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "first second third");
        assert_eq!(chunks[0].files.len(), 3);
        assert!((chunks[0].duration_secs - 15.0).abs() < 0.01);
    }

    // ── 8. gap splits into two chunks ─────────────────────────────────

    #[test]
    fn test_gap_splits_into_two_chunks() {
        // Second utterance starts 400s after first ends — exceeds 300s gap.
        // First: starts 09:00:00, duration 10s, ends at 09:00:10
        // Second: starts 09:07:00 (= 09:00:00 + 420s), gap = 420 - 10 = 410s > 300
        let transcripts = vec![
            make_transcript("mic", "mic_09-00-00.wav", "morning", 10.0),
            make_transcript("mic", "mic_09-07-00.wav", "later", 10.0),
        ];
        let chunks = chunk_transcripts(&transcripts, 300, 300);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, "morning");
        assert_eq!(chunks[1].text, "later");
    }

    // ── 9. source change splits chunk ─────────────────────────────────

    #[test]
    fn test_source_change_splits_chunk() {
        let transcripts = vec![
            make_transcript("mic", "mic_09-00-00.wav", "from mic", 5.0),
            make_transcript("teams", "teams_09-00-10.wav", "from teams", 5.0),
        ];
        let chunks = chunk_transcripts(&transcripts, 300, 300);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].source, "mic");
        assert_eq!(chunks[0].text, "from mic");
        assert_eq!(chunks[1].source, "teams");
        assert_eq!(chunks[1].text, "from teams");
    }

    // ── 10. max duration splits chunk ─────────────────────────────────

    #[test]
    fn test_max_duration_splits_chunk() {
        // 40 utterances * 10s each = 400s total audio.
        // With chunk_max_duration_secs=300, should split when accumulated >= 300s.
        let mut transcripts = Vec::new();
        for i in 0..40 {
            let minute = i / 4;
            let sec = (i % 4) * 15; // 15s apart to avoid gap splits
            let file = format!("mic_{:02}-{:02}-{:02}.wav", 9, minute, sec);
            transcripts.push(make_transcript("mic", &file, &format!("utt{}", i), 10.0));
        }

        let chunks = chunk_transcripts(&transcripts, 300, 300);
        assert!(
            chunks.len() >= 2,
            "Expected at least 2 chunks, got {}",
            chunks.len()
        );

        // All text should be accounted for.
        let total_text: String = chunks
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        for i in 0..40 {
            assert!(
                total_text.contains(&format!("utt{}", i)),
                "Missing utt{}",
                i
            );
        }
    }

    // ── 11. unsorted input is sorted by filename ──────────────────────

    #[test]
    fn test_unsorted_input_is_sorted_by_filename() {
        // Provide in reverse order — chunker should sort by filename.
        let transcripts = vec![
            make_transcript("mic", "mic_09-00-20.wav", "third", 5.0),
            make_transcript("mic", "mic_09-00-00.wav", "first", 5.0),
            make_transcript("mic", "mic_09-00-10.wav", "second", 5.0),
        ];
        let chunks = chunk_transcripts(&transcripts, 300, 300);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "first second third");
        assert_eq!(chunks[0].start_time, "09-00-00");
        assert_eq!(
            chunks[0].files,
            vec!["mic_09-00-00.wav", "mic_09-00-10.wav", "mic_09-00-20.wav",]
        );
    }

    // ── 12. chunk end time calculation ────────────────────────────────

    #[test]
    fn test_chunk_end_time_calculation() {
        // Start at 09:00:00, last utterance starts at 09:00:10 with 5s duration.
        // End time should be 09:00:15.
        let transcripts = vec![
            make_transcript("mic", "mic_09-00-00.wav", "a", 5.0),
            make_transcript("mic", "mic_09-00-10.wav", "b", 5.0),
        ];
        let chunks = chunk_transcripts(&transcripts, 300, 300);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_time, "09-00-00");
        assert_eq!(chunks[0].end_time, "09-00-15");
    }
}
