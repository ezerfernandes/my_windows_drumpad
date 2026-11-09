//! Folder-to-kit import acceptance tests.

use std::fs;
use std::path::Path;

use drumpad_model::kit::{
    KeyMapping, KitLibrary, MidiMapping, PAD_COUNT, PlaybackMode, VelocityRange,
};
use drumpad_samples::decode::decode_file;
use drumpad_samples::kit_import::{KitImportError, import_folder_into_new_kit};

fn write_wav(path: &Path, frequency: f64) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    let frames = 1_440;
    for frame in 0..frames {
        let value =
            (2.0 * std::f64::consts::PI * frequency * frame as f64 / 48_000.0).sin() * 8_000.0;
        writer.write_sample(value as i16).unwrap();
    }
    writer.finalize().unwrap();
}

fn library() -> (tempfile::TempDir, KitLibrary) {
    let dir = tempfile::tempdir().unwrap();
    let library = KitLibrary::open(dir.path()).unwrap();
    (dir, library)
}

fn hash_of(path: &Path) -> drumpad_model::kit::ContentHash {
    decode_file(path).unwrap().audio.content_hash()
}

#[test]
fn three_files_assign_pads_and_manifest_reloads() {
    let (dir, mut library) = library();
    let folder = dir.path().join("incoming");
    fs::create_dir_all(&folder).unwrap();
    for (name, frequency) in [("c.wav", 330.0), ("a.wav", 220.0), ("b.wav", 440.0)] {
        write_wav(&folder.join(name), frequency);
    }

    let imported = import_folder_into_new_kit(&mut library, "Imported Kit", &folder).unwrap();
    assert_eq!(imported.imported, 3);
    assert_eq!(imported.skipped, 0);
    assert!(imported.rejected.is_empty());
    assert_eq!(imported.kit.metadata.name, "Imported Kit");
    assert_eq!(imported.kit.pads.len(), PAD_COUNT);
    assert_eq!(library.active_kit_id(), Some(imported.kit.id));

    let expected_names = ["a", "b", "c"];
    for slot in 0..PAD_COUNT {
        let pad = &imported.kit.pads[slot];
        assert_eq!(pad.id.get(), slot as u8);
        if slot < expected_names.len() {
            assert_eq!(pad.name.as_deref(), Some(expected_names[slot]));
            assert_eq!(pad.layers.len(), 1);
            let sample = pad.layers[0].sample.as_ref().unwrap();
            assert!(sample.resolve(&imported.kit_dir).is_file());
            assert_eq!(
                sample.hash,
                Some(hash_of(
                    &folder.join(format!("{}.wav", expected_names[slot]))
                ))
            );
            assert_eq!(pad.key, KeyMapping::default_for(slot));
            assert_eq!(pad.midi, MidiMapping::default_for(slot));
            assert_eq!(pad.playback, PlaybackMode::OneShot);
            assert_eq!(pad.layers[0].velocity, VelocityRange::FULL);
        } else {
            assert!(pad.is_silent());
            assert!(pad.layers.is_empty());
            assert!(pad.key.is_none());
            assert!(pad.midi.is_none());
        }
    }

    let reloaded = library.load(imported.kit.id).unwrap();
    assert_eq!(reloaded.pads, imported.kit.pads);
    assert_eq!(reloaded.metadata.name, "Imported Kit");

    let copied = fs::read_dir(imported.kit_dir.join("samples"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(copied.len(), 3);
    for (index, name) in expected_names.iter().enumerate() {
        let sample = imported.kit.pads[index].layers[0].sample.as_ref().unwrap();
        assert_eq!(
            fs::read(sample.resolve(&imported.kit_dir)).unwrap(),
            fs::read(folder.join(format!("{name}.wav"))).unwrap()
        );
    }
}

#[test]
fn more_than_sixteen_files_assigns_the_first_sixteen_and_counts_skips() {
    let (dir, mut library) = library();
    let folder = dir.path().join("many");
    fs::create_dir_all(&folder).unwrap();
    for index in 0..18 {
        write_wav(
            &folder.join(format!("sample-{index:02}.wav")),
            200.0 + index as f64 * 10.0,
        );
    }

    let imported = import_folder_into_new_kit(&mut library, "Big Kit", &folder).unwrap();
    assert_eq!(imported.imported, PAD_COUNT);
    assert_eq!(imported.skipped, 2);
    assert!(imported.rejected.is_empty());
    assert_eq!(imported.kit.pads.len(), PAD_COUNT);
    assert_eq!(imported.kit.pads[0].name.as_deref(), Some("sample-00"));
    assert_eq!(imported.kit.pads[15].name.as_deref(), Some("sample-15"));
    assert!(imported.kit.pads.iter().all(|pad| !pad.is_silent()));
    assert_eq!(
        fs::read_dir(imported.kit_dir.join("samples"))
            .unwrap()
            .count(),
        PAD_COUNT
    );
}

#[test]
fn unsupported_and_corrupt_files_are_rejected_without_aborting() {
    let (dir, mut library) = library();
    let folder = dir.path().join("mixed");
    fs::create_dir_all(&folder).unwrap();
    write_wav(&folder.join("good.wav"), 440.0);
    fs::write(folder.join("broken.wav"), b"not audio at all").unwrap();
    fs::write(folder.join("empty.wav"), b"").unwrap();
    fs::write(folder.join("notes.txt"), b"not audio").unwrap();

    let imported = import_folder_into_new_kit(&mut library, "Mixed Kit", &folder).unwrap();
    assert_eq!(imported.imported, 1);
    assert_eq!(imported.skipped, 0);
    assert_eq!(imported.rejected.len(), 3);
    assert!(
        imported
            .rejected
            .iter()
            .all(|rejected| !rejected.reason.is_empty())
    );
    assert!(
        imported
            .rejected
            .iter()
            .any(|rejected| rejected.path.ends_with("notes.txt")
                && rejected.reason.contains("unsupported"))
    );
    assert!(
        imported
            .rejected
            .iter()
            .any(|rejected| rejected.path.ends_with("broken.wav"))
    );
    assert_eq!(imported.kit.pads[0].name.as_deref(), Some("good"));
    assert!(imported.kit.pads[1].is_silent());
    assert!(library.load(imported.kit.id).is_ok());
}

#[test]
fn duplicate_basenames_get_unique_copies() {
    let (dir, mut library) = library();
    let folder = dir.path().join("dupes");
    fs::create_dir_all(folder.join("one")).unwrap();
    fs::create_dir_all(folder.join("two")).unwrap();
    write_wav(&folder.join("one/kick.wav"), 220.0);
    write_wav(&folder.join("two/kick.wav"), 880.0);

    let imported = import_folder_into_new_kit(&mut library, "Dupes", &folder).unwrap();
    assert_eq!(imported.imported, 2);
    assert_eq!(imported.kit.pads[0].name.as_deref(), Some("kick"));
    assert_eq!(imported.kit.pads[1].name.as_deref(), Some("kick"));

    let first = imported.kit.pads[0].layers[0].sample.as_ref().unwrap();
    let second = imported.kit.pads[1].layers[0].sample.as_ref().unwrap();
    assert_ne!(first.path, second.path);
    assert_eq!(first.path.as_str(), "samples/pad-01-kick.wav");
    assert_eq!(second.path.as_str(), "samples/pad-02-kick.wav");
    assert_eq!(first.hash, Some(hash_of(&folder.join("one/kick.wav"))));
    assert_eq!(second.hash, Some(hash_of(&folder.join("two/kick.wav"))));
    assert!(first.resolve(&imported.kit_dir).is_file());
    assert!(second.resolve(&imported.kit_dir).is_file());
    assert_ne!(
        fs::read(first.resolve(&imported.kit_dir)).unwrap(),
        fs::read(second.resolve(&imported.kit_dir)).unwrap()
    );
}

#[test]
fn empty_folder_returns_a_typed_error_and_leaves_the_library_unchanged() {
    let (dir, mut library) = library();
    let folder = dir.path().join("empty");
    fs::create_dir_all(&folder).unwrap();
    let before = library.list().unwrap().len();
    let active = library.active_kit_id();

    let error = import_folder_into_new_kit(&mut library, "Empty", &folder).unwrap_err();
    assert!(matches!(
        error,
        KitImportError::NoValidFiles {
            rejected,
            ..
        } if rejected.is_empty()
    ));
    assert_eq!(library.list().unwrap().len(), before);
    assert_eq!(library.active_kit_id(), active);
}

#[test]
fn hidden_and_temp_files_are_ignored() {
    let (dir, mut library) = library();
    let folder = dir.path().join("hidden");
    fs::create_dir_all(&folder).unwrap();
    write_wav(&folder.join("good.wav"), 440.0);
    write_wav(&folder.join(".hidden.wav"), 300.0);
    write_wav(&folder.join("~$lock.wav"), 350.0);
    fs::write(folder.join("Thumbs.db"), b"cache").unwrap();
    fs::write(folder.join("desktop.ini"), b"config").unwrap();

    let imported = import_folder_into_new_kit(&mut library, "Visible", &folder).unwrap();
    assert_eq!(imported.imported, 1);
    assert_eq!(imported.skipped, 0);
    assert!(imported.rejected.is_empty());
    assert_eq!(imported.kit.pads[0].name.as_deref(), Some("good"));
}

#[test]
fn a_file_path_is_not_a_folder() {
    let (dir, mut library) = library();
    let file = dir.path().join("not-a-folder.wav");
    write_wav(&file, 440.0);
    let error = import_folder_into_new_kit(&mut library, "Nope", &file).unwrap_err();
    assert!(matches!(error, KitImportError::NotAFolder(_)));
}

#[test]
fn kit_dir_returns_the_created_folder_and_reports_missing_kits() {
    use drumpad_model::kit::{KitId, LibraryError};

    let (_dir, mut library) = library();
    let kit = library.create("Folder Lookup").unwrap();
    let dir = library.kit_dir(kit.id).unwrap();
    assert!(dir.is_dir());
    assert!(dir.join("kit.json").is_file());
    assert!(matches!(
        library.kit_dir(KitId::new()),
        Err(LibraryError::NotFound(_))
    ));
}
