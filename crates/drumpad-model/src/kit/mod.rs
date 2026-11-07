//! Kit domain, durable format, persistence, migration, repair, and bundles.

pub mod bundle;
pub mod demo;
pub mod format;
pub mod library;
pub mod migration;
pub mod model;
pub mod persistence;
pub mod repair;
pub mod state;
pub mod validation;

pub use bundle::{
    BundleError, ExportReport, ImportReport, export_active_kit, export_kit_archive,
    export_kit_folder, import_kit_archive, import_kit_folder,
};
pub use demo::{
    DEMO_KIT_JSON, DEMO_KIT_LICENSES, DEMO_MANIFEST, DEMO_SAMPLES, DemoError, demo_kit,
    demo_sample_bytes, install_demo_kit,
};
pub use format::{
    FormatError, KIT_V1_SCHEMA, ParsedKit, parse_kit_bytes, parse_kit_json, to_json,
    to_json_pretty, to_value,
};
pub use library::{BackupInfo, DeleteResult, KitLibrary, KitSummary, LibraryError};
pub use migration::{MigrationError, MigrationReport, migrate_value, read_version};
pub use model::{
    CURRENT_FORMAT_VERSION, ChokeGroup, ContentHash, DEFAULT_PAD_MIDI_NOTES, DEFAULT_PAD_SCANCODES,
    DomainError, KeyMapping, Kit, KitId, KitMetadata, LEGACY_FORMAT_VERSION, Layer, LayerId,
    MAX_CHOKE_GROUP, MAX_LAYERS_PER_PAD, MidiChannel, MidiMapping, MidiNote, PAD_COUNT, Pad, PadId,
    PhysicalKey, PlaybackMode, RelativeSamplePath, SampleLoop, SampleRef, VelocityRange,
    unix_time_ms,
};
pub use persistence::{
    AutosaveController, DEFAULT_AUTOSAVE_DEBOUNCE, DEFAULT_BACKUP_COUNT, LoadError, LoadedKit,
    SaveError, atomic_write, backup_before_migration, load_kit_file, rotate_backups,
    save_kit_atomic, save_kit_with_backup,
};
pub use repair::{RepairAction, RepairReport, repair_kit};
pub use state::{KitWorkspaceState, LoadStatus, StateError};
pub use validation::{
    Severity, ValidationCode, ValidationError, ValidationIssue, ValidationReport, validate_kit,
    validate_kit_report,
};
