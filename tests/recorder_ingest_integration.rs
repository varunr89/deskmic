//! End-to-end test for recorder ingestion against a stubbed device tree.

use std::path::PathBuf;

use deskmic::config::Config;
use deskmic::recorder_ingest;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/recorder/short_speech.mp3")
}

#[test]
fn ingest_end_to_end_against_stub_device() {
    use std::fs;

    let work = tempfile::TempDir::new().unwrap();
    let recordings = work.path().join("recordings");
    fs::create_dir_all(&recordings).unwrap();

    let device = work.path().join("device");
    let folder = device.join("REC_FILE").join("FOLDER01");
    fs::create_dir_all(&folder).unwrap();
    fs::copy(fixture(), folder.join("260429_0909.mp3")).unwrap();

    let db_path = recorder_ingest::recorder_db_path(&recordings);
    let conn = recorder_ingest::registry::open(&db_path).unwrap();

    let cfg = {
        let mut c = Config::default();
        c.output.directory = recordings.clone();
        c.recorder.volume_label = "FAKE".into();
        c.recorder.chunk_target_minutes = 1; // fixture is ~12 s; force 1 chunk
        c
    };

    let entries: Vec<_> = fs::read_dir(&folder).unwrap().collect();
    assert_eq!(entries.len(), 1);
    let path = entries.into_iter().next().unwrap().unwrap().path();
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let meta = fs::metadata(&path).unwrap();
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let parsed = recorder_ingest::filename::parse(&name).unwrap();

    recorder_ingest::ingest_one_for_test(
        &conn, &path, &name, size, mtime, &parsed, &recordings, &cfg,
    )
    .unwrap();

    let date_dir = recordings.join("2026-04-29");
    let mut wavs = 0;
    let mut sidecars = 0;
    let mut mp3_copies = 0;
    for e in fs::read_dir(&date_dir).unwrap() {
        let n = e.unwrap().file_name().to_string_lossy().to_string();
        if n.starts_with("recorder_") && n.ends_with(".wav") {
            wavs += 1;
        }
        if n.starts_with("recorder_") && n.ends_with(".json") {
            sidecars += 1;
        }
        if n.starts_with("recorder_") && n.ends_with(".mp3") {
            mp3_copies += 1;
        }
    }
    assert!(wavs >= 1, "expected at least one chunk wav, got {}", wavs);
    assert_eq!(wavs, sidecars, "wav and sidecar counts must match");
    assert_eq!(mp3_copies, 1, "exactly one local mp3 copy expected");

    assert!(recorder_ingest::registry::is_known(&conn, &name, size, mtime).unwrap());
}
