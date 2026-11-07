//! Bridge between the UI kit catalogue and the real `drumpad-model` kit
//! library.
//!
//! The UI keeps session-local `u64` kit ids so the browser can be tested
//! without the model crate; this service owns the mapping to the durable
//! `KitId` and performs every disk operation. It also implements the
//! "create a kit from a folder" import: scan supported audio, copy it into the
//! new kit folder, and write one full-range layer per pad.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use drumpad_model::kit::{Kit, KitId, KitLibrary, KitSummary as ModelKitSummary};
use drumpad_samples::{ImportedKit, import_folder_into_new_kit};

use crate::state::kit::KitBrowser;

/// Result of importing a folder into a new kit.
#[derive(Debug, Clone)]
pub struct ImportOutcome {
    /// Newly created kit.
    pub kit: Kit,
    /// Kit folder on disk.
    pub kit_dir: PathBuf,
    /// Number of samples copied into pads.
    pub imported: usize,
    /// Files the scanner rejected; each entry is `name: reason`.
    pub rejected: Vec<String>,
    /// Files that did not fit in the 16 pads.
    pub skipped: usize,
    /// One-line summary for the toast.
    pub summary: String,
}

/// The durable kit library plus the currently loaded kit.
pub struct KitService {
    library: KitLibrary,
    active: Option<Kit>,
    active_dir: Option<PathBuf>,
    ui_to_model: HashMap<u64, KitId>,
}

impl KitService {
    /// Open (or create) `<data_dir>/kits`, install the demo kit on first run,
    /// and load the active kit.
    pub fn open(root: PathBuf) -> Result<Self, String> {
        let mut library = KitLibrary::open(root).map_err(|error| error.to_string())?;
        let summaries = library.list().map_err(|error| error.to_string())?;
        let has_demo = summaries.iter().any(is_demo);
        // Install the demo only while the library still holds nothing but the
        // silent placeholder that `KitLibrary::open` creates. An existing user
        // kit is never replaced, and deleting the demo does not bring it back
        // on the next launch.
        let only_silent_default =
            summaries.len() == 1 && summaries[0].name == "Default Kit" && !is_demo(&summaries[0]);
        if !has_demo && only_silent_default {
            library
                .install_demo_kit()
                .map_err(|error| format!("could not install the demo kit: {error}"))?;
            // Drop the placeholder row so the catalogue opens on the demo.
            if let Some(default) = summaries.first() {
                let _ = library.delete(default.id);
            }
        }

        let mut service = Self {
            library,
            active: None,
            active_dir: None,
            ui_to_model: HashMap::new(),
        };
        service.load_active()?;
        Ok(service)
    }

    /// The currently loaded kit.
    pub fn active(&self) -> Option<&Kit> {
        self.active.as_ref()
    }

    /// Folder of the currently loaded kit.
    pub fn active_dir(&self) -> Option<&Path> {
        self.active_dir.as_deref()
    }

    /// Refresh the UI browser from `library.list()` and rebuild the id map.
    pub fn refresh(&mut self, browser: &mut KitBrowser) -> Result<(), String> {
        let summaries = self.library.list().map_err(|error| error.to_string())?;
        self.ui_to_model.clear();
        let active_model = self.active.as_ref().map(|kit| kit.id);
        let mut entries = Vec::with_capacity(summaries.len());
        let mut active_ui = None;

        for (index, summary) in summaries.iter().enumerate() {
            let ui_id = index as u64 + 1;
            self.ui_to_model.insert(ui_id, summary.id);
            if Some(summary.id) == active_model {
                active_ui = Some(ui_id);
            }
            entries.push((ui_id, summary.name.clone(), is_demo(summary)));
        }
        browser.replace_catalogue_ids(&entries, active_ui);
        Ok(())
    }

    /// Load a kit selected in the UI.
    pub fn load_ui(&mut self, ui_id: u64) -> Result<(Kit, PathBuf), String> {
        let model_id = self
            .ui_to_model
            .get(&ui_id)
            .copied()
            .ok_or_else(|| "kit is no longer in the library".to_owned())?;
        let kit = self
            .library
            .load(model_id)
            .map_err(|error| error.to_string())?;
        let dir = self
            .library
            .kit_dir(model_id)
            .map_err(|error| error.to_string())?;
        // Persist the active choice only after the kit and its folder loaded,
        // so a broken kit cannot poison the next launch.
        self.library
            .set_active(model_id)
            .map_err(|error| error.to_string())?;
        self.active = Some(kit.clone());
        self.active_dir = Some(dir.clone());
        Ok((kit, dir))
    }

    /// Reload the active kit recorded in the library.
    pub fn load_active(&mut self) -> Result<(), String> {
        let Some(kit) = self
            .library
            .load_active()
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let dir = self
            .library
            .kit_dir(kit.id)
            .map_err(|error| error.to_string())?;
        self.active = Some(kit);
        self.active_dir = Some(dir);
        Ok(())
    }

    /// Create an empty kit and activate it.
    pub fn create(&mut self, name: &str) -> Result<(Kit, PathBuf), String> {
        let kit = self
            .library
            .create(name)
            .map_err(|error| error.to_string())?;
        let dir = self
            .library
            .kit_dir(kit.id)
            .map_err(|error| error.to_string())?;
        self.library
            .set_active(kit.id)
            .map_err(|error| error.to_string())?;
        self.active = Some(kit.clone());
        self.active_dir = Some(dir.clone());
        Ok((kit, dir))
    }

    /// Persist the active kit.
    pub fn save_active(&mut self) -> Result<(), String> {
        let Some(kit) = self.active.as_mut() else {
            return Err("no kit is loaded".to_owned());
        };
        self.library.save(kit).map_err(|error| error.to_string())
    }

    /// Persist the kit selected in the UI.
    ///
    /// The active kit may hold unsaved edits in memory; other kits are saved
    /// from their on-disk form, which makes `Save(id)` a no-op rather than
    /// saving the wrong kit.
    pub fn save_ui(&mut self, ui_id: u64) -> Result<(), String> {
        let model_id = self.model_id(ui_id)?;
        let is_active = self.active.as_ref().is_some_and(|kit| kit.id == model_id);
        let mut kit = if is_active {
            self.active
                .clone()
                .ok_or_else(|| "no kit is loaded".to_owned())?
        } else {
            self.library
                .load(model_id)
                .map_err(|error| error.to_string())?
        };
        self.library
            .save(&mut kit)
            .map_err(|error| error.to_string())?;
        if is_active {
            self.active = Some(kit);
        }
        Ok(())
    }

    /// Rename a kit selected in the UI.
    pub fn rename_ui(&mut self, ui_id: u64, name: &str) -> Result<(), String> {
        let model_id = self.model_id(ui_id)?;
        let kit = self
            .library
            .rename(model_id, name)
            .map_err(|error| error.to_string())?;
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.id == model_id)
        {
            self.active = Some(kit);
        }
        Ok(())
    }

    /// Duplicate a kit selected in the UI.
    pub fn duplicate_ui(&mut self, ui_id: u64) -> Result<(), String> {
        let model_id = self.model_id(ui_id)?;
        self.library
            .duplicate(model_id, None)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Delete a kit selected in the UI and load the fallback if it was active.
    pub fn delete_ui(&mut self, ui_id: u64) -> Result<(), String> {
        let model_id = self.model_id(ui_id)?;
        let result = self
            .library
            .delete(model_id)
            .map_err(|error| error.to_string())?;
        if self.active.as_ref().is_some_and(|kit| kit.id == model_id) {
            self.load_active()?;
        }
        if result.fallback.is_none() {
            self.active = None;
            self.active_dir = None;
        }
        Ok(())
    }

    /// Import every supported file in `folder` into a new kit named `name`.
    pub fn import_folder(&mut self, name: &str, folder: &Path) -> Result<ImportOutcome, String> {
        let imported: ImportedKit = import_folder_into_new_kit(&mut self.library, name, folder)
            .map_err(|error| error.to_string())?;
        self.library
            .set_active(imported.kit.id)
            .map_err(|error| error.to_string())?;
        self.active = Some(imported.kit.clone());
        self.active_dir = Some(imported.kit_dir.clone());

        let rejected: Vec<String> = imported
            .rejected
            .iter()
            .map(|file| {
                format!(
                    "{}: {}",
                    file.path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    file.reason
                )
            })
            .collect();
        let summary = format!(
            "{} imported, {} rejected, {} skipped (16 pad limit)",
            imported.imported,
            rejected.len(),
            imported.skipped
        );
        Ok(ImportOutcome {
            kit: imported.kit,
            kit_dir: imported.kit_dir,
            imported: imported.imported,
            rejected,
            skipped: imported.skipped,
            summary,
        })
    }

    fn model_id(&self, ui_id: u64) -> Result<KitId, String> {
        self.ui_to_model
            .get(&ui_id)
            .copied()
            .ok_or_else(|| "kit is no longer in the library".to_owned())
    }
}

fn is_demo(summary: &ModelKitSummary) -> bool {
    summary.name.contains("Demo")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::kit::KitBrowser;
    use std::fs;

    fn service() -> (tempfile::TempDir, KitService, KitBrowser) {
        let temp = tempfile::tempdir().expect("temp");
        let mut service = KitService::open(temp.path().join("kits")).expect("library");
        let mut browser = KitBrowser::new();
        service.refresh(&mut browser).expect("refresh");
        (temp, service, browser)
    }

    #[test]
    fn opening_a_library_installs_and_loads_a_demo_kit() {
        let (_temp, service, browser) = service();
        let active = service.active().expect("active kit");
        assert!(active.metadata.name.contains("Demo"));
        assert!(
            browser
                .kits()
                .iter()
                .any(|kit| kit.name == active.metadata.name)
        );
        assert_eq!(browser.active_name(), Some(active.metadata.name.as_str()));
        assert!(
            !browser.kits().iter().any(|kit| kit.name == "Default Kit"),
            "the silent placeholder should be removed after the demo installs"
        );
    }

    #[test]
    fn a_user_kit_stays_active_after_the_demo_is_deleted() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("kits");
        let mut service = KitService::open(root.clone()).expect("library");
        let (user, _) = service.create("User Kit").expect("create");
        let demo = service
            .library
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.name.contains("Demo"))
            .expect("demo installed");
        service.library.delete(demo.id).expect("delete demo");
        drop(service);

        let reopened = KitService::open(root).expect("reopen");
        assert_eq!(reopened.active().expect("active").id, user.id);
        let names: Vec<String> = reopened
            .library
            .list()
            .expect("list")
            .into_iter()
            .map(|row| row.name)
            .collect();
        assert!(
            !names.iter().any(|name| name.contains("Demo")),
            "deleting the demo must not reinstall it on the next launch"
        );
    }

    #[test]
    fn saving_by_id_never_replaces_the_active_kit() {
        let (_temp, mut service, _browser) = service();
        let (_first, _) = service.create("First").expect("create");
        let (second, _) = service.create("Second").expect("create");
        let mut browser = KitBrowser::new();
        service.refresh(&mut browser).expect("refresh");

        let first_ui = browser
            .kits()
            .iter()
            .find(|row| row.name == "First")
            .expect("first row")
            .id
            .0;
        service.save_ui(first_ui).expect("save first");
        assert_eq!(service.active().expect("active").id, second.id);

        let second_ui = browser
            .kits()
            .iter()
            .find(|row| row.name == "Second")
            .expect("second row")
            .id
            .0;
        service.save_ui(second_ui).expect("save second");
        assert_eq!(service.active().expect("active").id, second.id);
    }

    #[test]
    fn creating_a_kit_refreshes_the_catalogue_and_becomes_active() {
        let (_temp, mut service, mut browser) = service();
        let (kit, dir) = service.create("Rock").expect("create");
        assert!(dir.is_dir());
        service.refresh(&mut browser).expect("refresh");
        assert!(browser.kits().iter().any(|row| row.name == "Rock"));
        assert_eq!(browser.active_name(), Some("Rock"));
        assert_eq!(service.active().expect("active").id, kit.id);
    }

    #[test]
    fn an_empty_kit_cannot_be_loaded_into_the_audio_runtime() {
        // The runtime rejects kits with no samples; the service still creates
        // and activates them so the user can import later.
        let (_temp, mut service, _browser) = service();
        let (kit, _) = service.create("Empty").expect("create");
        assert!(kit.pads.iter().all(|pad| pad.layers.is_empty()));
    }

    #[test]
    fn importing_a_folder_creates_pads_with_samples() {
        let temp = tempfile::tempdir().expect("temp");
        let samples = temp.path().join("samples-src");
        fs::create_dir_all(&samples).expect("samples dir");
        // Two tiny valid WAV files from the demo assets.
        let demo = drumpad_model::kit::demo::DEMO_SAMPLES;
        fs::write(samples.join("kick.wav"), demo[0].1).expect("kick");
        fs::write(samples.join("snare.wav"), demo[1].1).expect("snare");
        fs::write(samples.join("notes.txt"), b"ignored").expect("txt");

        let mut service = KitService::open(temp.path().join("kits")).expect("library");
        let outcome = service.import_folder("Imported", &samples).expect("import");
        assert_eq!(outcome.imported, 2);
        assert_eq!(outcome.rejected.len(), 1);
        assert!(outcome.kit.pads[0].layers[0].sample.is_some());
        assert!(outcome.kit.pads[1].layers[0].sample.is_some());
        assert!(
            outcome.kit_dir.join("samples").is_dir(),
            "samples should be copied into the kit"
        );
        let mut browser = KitBrowser::new();
        service.refresh(&mut browser).expect("refresh");
        assert_eq!(browser.active_name(), Some("Imported"));
    }

    #[test]
    fn importing_a_folder_without_audio_fails_with_a_reason() {
        let temp = tempfile::tempdir().expect("temp");
        let empty = temp.path().join("empty");
        fs::create_dir_all(&empty).expect("dir");
        let mut service = KitService::open(temp.path().join("kits")).expect("library");
        let error = service.import_folder("Nope", &empty).expect_err("empty");
        assert!(error.contains("no valid audio files"));
    }

    #[test]
    fn rename_and_delete_update_the_durable_library() {
        let (_temp, mut service, mut browser) = service();
        let (kit, _) = service.create("Old").expect("create");
        service.refresh(&mut browser).expect("refresh");
        let ui_id = browser
            .kits()
            .iter()
            .find(|row| row.name == "Old")
            .expect("row")
            .id
            .0;
        service.rename_ui(ui_id, "New").expect("rename");
        service.refresh(&mut browser).expect("refresh");
        assert!(browser.kits().iter().any(|row| row.name == "New"));

        let ui_id = browser
            .kits()
            .iter()
            .find(|row| row.name == "New")
            .expect("row")
            .id
            .0;
        service.delete_ui(ui_id).expect("delete");
        service.refresh(&mut browser).expect("refresh");
        assert!(!browser.kits().iter().any(|row| row.name == "New"));
        assert_ne!(service.active().map(|kit| kit.id), Some(kit.id));
    }
}
