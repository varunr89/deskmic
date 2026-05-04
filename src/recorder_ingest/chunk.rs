use std::path::{Path, PathBuf};

use anyhow::Result;
use hound::{WavSpec, WavWriter};
use serde::{Deserialize, Serialize};

use super::decode::TARGET_RATE;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkSidecar {
    pub recording_id: String,
    pub chunk_index: u32,
    pub base_offset_secs: f64,
}

pub struct ChunkPlan {
    pub start_sample: usize,
    pub end_sample: usize,
    pub base_offset_secs: f64,
}

/// Coarse silence trim: drop frames whose RMS falls below `threshold`,
/// applying a `hangover_ms` window so natural pauses stay inside speech.
pub fn vad_trim(samples: &[f32], hangover_ms: u32, threshold: f32) -> Vec<f32> {
    let frame_size = TARGET_RATE as usize / 100;
    let hangover_frames = (hangover_ms / 10).max(1) as usize;

    let mut voiced: Vec<bool> = samples
        .chunks(frame_size)
        .map(|f| {
            let rms = (f.iter().map(|s| s * s).sum::<f32>() / f.len().max(1) as f32).sqrt();
            rms > threshold
        })
        .collect();

    let n = voiced.len();
    let raw = voiced.clone();
    for i in 0..n {
        if !raw[i] {
            let lo = i.saturating_sub(hangover_frames);
            let hi = (i + hangover_frames + 1).min(n);
            if raw[lo..hi].iter().any(|&v| v) {
                voiced[i] = true;
            }
        }
    }

    let mut out = Vec::with_capacity(samples.len());
    for (i, frame) in samples.chunks(frame_size).enumerate() {
        if voiced.get(i).copied().unwrap_or(false) {
            out.extend_from_slice(frame);
        }
    }
    out
}

pub fn plan_chunks(samples: &[f32], target_secs: u32) -> Vec<ChunkPlan> {
    if samples.is_empty() {
        return Vec::new();
    }
    let target = (TARGET_RATE as usize) * target_secs as usize;
    let mut plans = Vec::new();
    let mut i = 0;
    while i < samples.len() {
        let end = (i + target).min(samples.len());
        plans.push(ChunkPlan {
            start_sample: i,
            end_sample: end,
            base_offset_secs: i as f64 / TARGET_RATE as f64,
        });
        i = end;
    }
    plans
}

pub fn write_chunk_wav(samples: &[f32], out_path: &Path) -> Result<()> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: TARGET_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = WavWriter::create(out_path, spec)?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        w.write_sample(v)?;
    }
    w.finalize()?;
    Ok(())
}

pub fn write_sidecar(path: &Path, sidecar: &ChunkSidecar) -> Result<()> {
    let json = serde_json::to_string_pretty(sidecar)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn chunk_basename(start_hhmmss: &str, chunk_index: u32) -> String {
    format!("recorder_{}_chunk{:02}", start_hhmmss, chunk_index)
}

pub fn date_dir_for(date: chrono::NaiveDate, recordings_root: &Path) -> PathBuf {
    recordings_root.join(date.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_trim_drops_silence() {
        let mut s = vec![0.0_f32; TARGET_RATE as usize];
        s.extend(std::iter::repeat(0.5_f32).take(TARGET_RATE as usize));
        s.extend(std::iter::repeat(0.0_f32).take(TARGET_RATE as usize));
        let trimmed = vad_trim(&s, 200, 0.05);
        let kept_secs = trimmed.len() as f32 / TARGET_RATE as f32;
        assert!(kept_secs > 0.8 && kept_secs < 1.6, "got {}", kept_secs);
    }

    #[test]
    fn plan_chunks_splits_evenly() {
        let s = vec![0.0_f32; (TARGET_RATE as usize) * 25];
        let plans = plan_chunks(&s, 10);
        assert_eq!(plans.len(), 3);
        assert_eq!(plans[0].base_offset_secs, 0.0);
        assert!((plans[1].base_offset_secs - 10.0).abs() < 1e-6);
        assert!((plans[2].base_offset_secs - 20.0).abs() < 1e-6);
    }

    #[test]
    fn plan_chunks_empty_returns_empty() {
        let plans = plan_chunks(&[], 10);
        assert!(plans.is_empty());
    }

    #[test]
    fn chunk_basename_zero_pads() {
        assert_eq!(chunk_basename("090900", 0), "recorder_090900_chunk00");
        assert_eq!(chunk_basename("090900", 7), "recorder_090900_chunk07");
    }

    #[test]
    fn sidecar_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("a.json");
        let s = ChunkSidecar { recording_id: "260429_0909.mp3".into(), chunk_index: 2, base_offset_secs: 600.0 };
        write_sidecar(&p, &s).unwrap();
        let back: ChunkSidecar = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(back.recording_id, s.recording_id);
        assert_eq!(back.chunk_index, 2);
        assert!((back.base_offset_secs - 600.0).abs() < 1e-6);
    }
}
