use std::path::Path;

use anyhow::Result;
use pyannote_rs::{EmbeddingExtractor, EmbeddingManager, get_segments};

use crate::transcribe::backend::Transcript;

/// Run speaker diarization on a list of transcripts from a teams audio file.
/// Assigns speaker labels ("Speaker 1", "Speaker 2", etc.) based on voice embeddings.
pub fn diarize_teams_transcripts(
    transcripts: &mut [Transcript],
    audio_path: &Path,
    segmentation_model: &Path,
    embedding_model: &Path,
) -> Result<()> {
    if transcripts.is_empty() {
        return Ok(());
    }

    // Read the raw audio samples
    let mut reader = hound::WavReader::open(audio_path)?;
    let spec = reader.spec();
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<std::result::Result<Vec<_>, _>>()?;

    // Get speech segments from pyannote segmentation model
    let segments: Vec<_> = get_segments(&samples, spec.sample_rate, segmentation_model)
        .map_err(|e| anyhow::anyhow!("Segmentation failed: {:?}", e))?
        .filter_map(|s| s.ok())
        .collect();

    if segments.is_empty() {
        // No speech segments found — label all as "Others"
        for t in transcripts.iter_mut() {
            t.speaker = Some("Others".to_string());
        }
        return Ok(());
    }

    // Extract embeddings for each speech segment and cluster into speakers
    let mut extractor = EmbeddingExtractor::new(embedding_model)
        .map_err(|e| anyhow::anyhow!("Failed to load embedding model: {:?}", e))?;
    let mut manager = EmbeddingManager::new(10); // up to 10 speakers

    let mut segment_speakers = Vec::new();
    for seg in &segments {
        match extractor.compute(&seg.samples) {
            Ok(embedding_iter) => {
                let embedding: Vec<f32> = embedding_iter.collect();
                let speaker_id = manager.search_speaker(embedding, 0.5).unwrap_or(0);
                segment_speakers.push((seg.start, seg.end, speaker_id));
            }
            Err(e) => {
                tracing::warn!("Failed to compute embedding for segment [{:.1}-{:.1}]: {:?}", seg.start, seg.end, e);
                segment_speakers.push((seg.start, seg.end, 0));
            }
        }
    }

    // Assign speaker labels to transcripts by matching timestamps
    for t in transcripts.iter_mut() {
        let t_start = t.start_secs.unwrap_or(0.0);
        let t_end = t.end_secs.unwrap_or(t_start + t.duration_secs);
        let t_mid = (t_start + t_end) / 2.0;

        // Find the diarization segment whose time range overlaps most with this transcript
        let mut best_speaker = 0usize;
        let mut best_overlap = 0.0f64;

        for &(seg_start, seg_end, speaker_id) in &segment_speakers {
            let overlap_start = t_start.max(seg_start);
            let overlap_end = t_end.min(seg_end);
            let overlap = (overlap_end - overlap_start).max(0.0);

            if overlap > best_overlap {
                best_overlap = overlap;
                best_speaker = speaker_id;
            }
        }

        // If no overlap found, use the closest segment by midpoint
        if best_overlap <= 0.0 {
            let mut min_dist = f64::MAX;
            for &(seg_start, seg_end, speaker_id) in &segment_speakers {
                let seg_mid = (seg_start + seg_end) / 2.0;
                let dist = (t_mid - seg_mid).abs();
                if dist < min_dist {
                    min_dist = dist;
                    best_speaker = speaker_id;
                }
            }
        }

        t.speaker = Some(format!("Speaker {}", best_speaker));
    }

    Ok(())
}

/// Check if diarization models are available locally.
pub fn models_available(segmentation_model: &Path, embedding_model: &Path) -> bool {
    segmentation_model.exists() && embedding_model.exists()
}
