//! End-to-end acceptance tests for the kit format and library workflows.

use std::fs;

use drumpad_model::kit::{
    KIT_V1_SCHEMA, Kit, KitLibrary, parse_kit_json, to_json_pretty, validate_kit_report,
};
use serde_json::Value;

fn manifest_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/demo-kit/kit.json")
}

fn demo_manifest() -> Value {
    serde_json::from_slice(&fs::read(manifest_path()).unwrap()).unwrap()
}

#[test]
fn example_kit_validates_against_the_json_schema() {
    let schema: Value = serde_json::from_str(KIT_V1_SCHEMA).unwrap();
    let validator = jsonschema::validator_for(&schema).expect("schema must compile");
    let instance = demo_manifest();
    let errors = validator.iter_errors(&instance).into_errors();
    assert!(
        errors.is_empty(),
        "demo kit violates the v1 schema: {errors}"
    );
    assert!(validator.is_valid(&instance));

    // And the domain invariants agree with the schema.
    let kit = parse_kit_json(&fs::read_to_string(manifest_path()).unwrap())
        .unwrap()
        .kit;
    let report = validate_kit_report(&kit);
    assert!(report.is_valid(), "{:#?}", report.issues);
}

#[test]
fn schema_rejects_invalid_pad_values() {
    let schema: Value = serde_json::from_str(KIT_V1_SCHEMA).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let mut instance = demo_manifest();
    instance["pads"][0]["pan"] = serde_json::json!(2.0);
    assert!(!validator.is_valid(&instance));

    let mut instance = demo_manifest();
    instance["pads"] = serde_json::json!([]);
    assert!(!validator.is_valid(&instance));
}

#[test]
fn unknown_fields_are_ignored_at_every_level() {
    let mut instance = demo_manifest();
    instance.as_object_mut().unwrap().insert(
        "future_top_level".into(),
        serde_json::json!({"note": "ignored"}),
    );
    instance["metadata"]
        .as_object_mut()
        .unwrap()
        .insert("future_metadata".into(), serde_json::json!(1));
    instance["pads"][0]
        .as_object_mut()
        .unwrap()
        .insert("future_pad".into(), serde_json::json!(true));
    instance["pads"][0]["layers"][0]
        .as_object_mut()
        .unwrap()
        .insert("future_layer".into(), serde_json::json!("ok"));

    let parsed = parse_kit_json(&serde_json::to_string(&instance).unwrap()).unwrap();
    assert_eq!(parsed.kit.pads.len(), 16);
    assert!(parsed.kit.validate().is_ok());
}

#[test]
fn future_kit_version_is_refused() {
    let mut instance = demo_manifest();
    instance["format_version"] = serde_json::json!(2);
    let error = parse_kit_json(&serde_json::to_string(&instance).unwrap()).unwrap_err();
    assert!(error.to_string().contains("newer"));
}

#[test]
fn legacy_kit_fixture_migrates_forward() {
    let legacy = serde_json::json!({
        "version": 0,
        "name": "Legacy Fixture",
        "author": "Test",
        "pads": [
            {"slot": 0, "sample_path": "samples/kick.wav", "midi_note": 36},
            {"slot": 1, "sample_path": "samples/snare.wav", "key_code": 3}
        ]
    });
    let parsed = parse_kit_json(&serde_json::to_string(&legacy).unwrap()).unwrap();
    let report = parsed.migration.expect("v0 should migrate");
    assert_eq!(report.from_version, 0);
    let kit = parsed.kit;
    assert_eq!(kit.format_version, 1);
    assert_eq!(
        kit.pads[0].layers[0].sample.as_ref().unwrap().path.as_str(),
        "samples/kick.wav"
    );
    assert_eq!(kit.pads[1].key.unwrap().key.raw(), 3);
    assert!(kit.validate().is_ok());
}

#[test]
fn library_round_trips_a_kit_and_its_backups() {
    let dir = tempfile::tempdir().unwrap();
    let mut library = KitLibrary::open(dir.path()).unwrap();
    let mut kit = library.create("Contract Kit").unwrap();
    for index in 0..3 {
        kit.metadata.description = Some(format!("revision {index}"));
        library.save(&mut kit).unwrap();
    }
    let loaded = library.load(kit.id).unwrap();
    assert_eq!(loaded, kit);

    let kit_dir = library.kit_dir(kit.id).unwrap();
    assert!(kit_dir.join(".backups/kit.bak1.json").is_file());

    // A fresh library over the same folder sees the same kit.
    let reopened = KitLibrary::open(dir.path()).unwrap();
    assert_eq!(reopened.load(kit.id).unwrap().metadata.name, "Contract Kit");
}

#[test]
fn bundle_export_and_import_reproduce_a_kit() {
    let work = tempfile::tempdir().unwrap();
    let source_dir = work.path().join("source");
    fs::create_dir_all(source_dir.join("samples")).unwrap();
    let sample = b"RIFF test data";
    fs::write(source_dir.join("samples/kick.wav"), sample).unwrap();

    let mut kit = Kit::silent_default();
    kit.metadata.name = "Portable".into();
    kit.pads[0]
        .layers
        .push(drumpad_model::kit::Layer::with_sample(
            Default::default(),
            drumpad_model::kit::SampleRef::new(
                drumpad_model::kit::RelativeSamplePath::new("samples/kick.wav").unwrap(),
                drumpad_model::kit::ContentHash::of_bytes(sample),
            ),
        ));
    fs::write(source_dir.join("kit.json"), to_json_pretty(&kit).unwrap()).unwrap();

    let bundle = work.path().join("bundle");
    drumpad_model::kit::export_kit_folder(&kit, &source_dir, &bundle).unwrap();
    let report =
        drumpad_model::kit::import_kit_folder(&bundle, &work.path().join("imported")).unwrap();
    assert_eq!(report.kit.pads, kit.pads);
    assert!(report.missing.is_empty());
    assert_eq!(
        fs::read(work.path().join("imported/samples/kick.wav")).unwrap(),
        sample
    );

    let archive = work.path().join("portable.zip");
    drumpad_model::kit::export_kit_archive(&kit, &source_dir, &archive).unwrap();
    let report =
        drumpad_model::kit::import_kit_archive(&archive, &work.path().join("zipped")).unwrap();
    assert_eq!(report.kit.pads, kit.pads);
}

#[test]
fn demo_kit_installs_and_can_be_removed() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("demo");
    let kit = drumpad_model::kit::install_demo_kit(&dest).unwrap();
    assert_eq!(kit.pads.len(), 16);
    assert!(dest.join("LICENSES.md").is_file());
    assert!(dest.join("samples/kick.wav").is_file());

    // Deleting the demo kit must not break the library.
    let mut library = KitLibrary::open(dir.path().join("library")).unwrap();
    let installed = library.create("Installed Demo").unwrap();
    library.delete(installed.id).unwrap();
    assert!(library.load_active().unwrap().is_some());
}
