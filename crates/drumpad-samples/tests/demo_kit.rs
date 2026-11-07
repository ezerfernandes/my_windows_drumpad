//! Cross-crate check: the embedded demo kit decodes with the real sample
//! pipeline and its manifest hashes match the decoded PCM.

use std::path::PathBuf;

use drumpad_samples::decode::decode_file;

#[test]
fn demo_kit_samples_decode_and_hashes_match() {
    let kit = drumpad_model::kit::demo_kit().unwrap();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(kit.samples().len(), 16);

    for sample in kit.samples() {
        let bytes = drumpad_model::kit::demo_sample_bytes(sample.path.as_str())
            .unwrap_or_else(|| panic!("embedded asset {} is missing", sample.path));
        let path: PathBuf = dir.path().join(sample.path.as_str().replace('/', "_"));
        std::fs::write(&path, bytes).unwrap();
        let decoded = decode_file(&path)
            .unwrap_or_else(|error| panic!("{} failed to decode: {error}", sample.path));
        assert_eq!(decoded.audio.sample_rate, 48_000, "{}", sample.path);
        assert_eq!(decoded.audio.channels, 1, "{}", sample.path);
        assert_eq!(
            sample.hash,
            Some(decoded.audio.content_hash()),
            "manifest hash for {} does not match decoded content",
            sample.path
        );
        assert!(decoded.audio.duration_seconds() > 0.05);
        assert!(!decoded.audio.is_empty());
    }
}

#[test]
fn demo_kit_imports_through_the_cache_and_resamples() {
    use std::sync::Arc;

    use drumpad_samples::cache::SampleCache;
    use drumpad_samples::resample::resample;

    let kit = drumpad_model::kit::demo_kit().unwrap();
    let cache = SampleCache::new(tempfile::tempdir().unwrap().path()).unwrap();
    let dir = tempfile::tempdir().unwrap();

    for sample in kit.samples() {
        let bytes = drumpad_model::kit::demo_sample_bytes(sample.path.as_str()).unwrap();
        let path = dir.path().join(sample.path.as_str().replace('/', "_"));
        std::fs::write(&path, bytes).unwrap();
        let decoded = decode_file(&path).unwrap();
        let stored = Arc::new(cache.insert(&decoded.audio).unwrap());
        assert_eq!(stored.metadata.hash, decoded.audio.content_hash());

        let resampled = resample(&stored.audio, 44_100).unwrap();
        assert_eq!(resampled.sample_rate, 44_100);
        assert!((resampled.duration_seconds() - decoded.audio.duration_seconds()).abs() < 0.01);
    }
}
