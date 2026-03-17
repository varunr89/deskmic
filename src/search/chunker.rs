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

/// Chunk a list of transcripts into conversation segments.
pub fn chunk_transcripts(
    _transcripts: &[Transcript],
    _chunk_gap_secs: u64,
    _chunk_max_duration_secs: u64,
) -> Vec<Chunk> {
    todo!("chunk_transcripts")
}
