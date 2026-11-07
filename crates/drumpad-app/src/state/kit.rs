//! Kit browser state: the in-memory catalogue, CRUD rules, and an asynchronous
//! load model.
//!
//! The browser never performs I/O. It produces [`KitRequest`] values that the
//! composition layer hands to `drumpad-model`'s kit service, and it consumes
//! progress and completion callbacks. That is what keeps a kit load off the UI
//! thread: the window keeps painting while a load is in flight, and the load
//! state machine is unit-tested without a filesystem.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stable kit identifier inside the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KitId(pub u64);

impl fmt::Display for KitId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "kit-{}", self.0)
    }
}

/// One row in the kit list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KitSummary {
    /// Catalogue id.
    pub id: KitId,
    /// Display name, unique within the catalogue.
    pub name: String,
    /// Manifest path, when the kit is on disk.
    pub path: Option<PathBuf>,
    /// Whether the kit has unsaved changes.
    pub dirty: bool,
    /// Factory kits cannot be deleted.
    pub factory: bool,
}

/// Outcome of an asynchronous load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadStatus {
    /// Nothing is loading.
    Idle,
    /// A load is in flight.
    Loading {
        /// Kit being loaded.
        kit: KitId,
        /// `0..=100`, published by the loader.
        percent: u8,
    },
    /// The kit is ready and active.
    Ready {
        /// The active kit.
        kit: KitId,
    },
    /// The last load failed.
    Failed {
        /// Kit that failed to load.
        kit: KitId,
        /// User-facing reason.
        message: String,
    },
}

impl LoadStatus {
    /// Whether a load is in flight.
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }
}

/// A request for the composition layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KitRequest {
    /// Load the kit's manifest and samples.
    Load(KitId),
    /// Persist the kit to disk.
    Save(KitId),
    /// Delete the kit's files.
    Delete(KitId),
    /// Create a new kit directory and manifest.
    Create(KitId),
    /// Rename the kit's manifest.
    Rename(KitId),
}

/// An action waiting on the unsaved-changes prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingAction {
    /// Switch to another kit.
    Switch(KitId),
    /// Delete a kit.
    Delete(KitId),
}

/// Why the browser refused an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KitError {
    /// Names cannot be empty.
    EmptyName,
    /// Names must be unique.
    DuplicateName(String),
    /// The kit does not exist.
    UnknownKit(KitId),
    /// Factory kits are read-only.
    FactoryReadOnly(KitId),
    /// The catalogue must keep at least one kit.
    LastKit,
    /// A load is already running; the request was not started.
    Busy,
    /// A load failed and needs an explicit retry or dismiss.
    LoadFailed,
}

impl fmt::Display for KitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => formatter.write_str("kit name cannot be empty"),
            Self::DuplicateName(name) => write!(formatter, "a kit named {name:?} already exists"),
            Self::UnknownKit(id) => write!(formatter, "kit {id} does not exist"),
            Self::FactoryReadOnly(id) => write!(formatter, "kit {id} is a factory kit and cannot be deleted"),
            Self::LastKit => formatter.write_str("at least one kit must remain"),
            Self::Busy => formatter.write_str("a kit load is already running"),
            Self::LoadFailed => formatter.write_str("a kit load failed; retry or dismiss it first"),
        }
    }
}

impl std::error::Error for KitError {}

/// The kit browser.
#[derive(Debug, Clone)]
pub struct KitBrowser {
    kits: Vec<KitSummary>,
    active: Option<KitId>,
    next_id: u64,
    load: LoadStatus,
    search: String,
    pending: Option<PendingAction>,
}

impl Default for KitBrowser {
    fn default() -> Self {
        Self::new()
    }
}

impl KitBrowser {
    /// An empty browser. Call [`Self::seed_factory`] or [`Self::create`] to add
    /// kits.
    pub fn new() -> Self {
        Self {
            kits: Vec::new(),
            active: None,
            next_id: 1,
            load: LoadStatus::Idle,
            search: String::new(),
            pending: None,
        }
    }

    /// A browser with the shipped factory kit, active and ready.
    pub fn with_factory_kit() -> Self {
        let mut browser = Self::new();
        let id = browser.insert("Default Kit", None, true);
        browser.active = Some(id);
        browser.load = LoadStatus::Ready { kit: id };
        browser
    }

    fn insert(&mut self, name: &str, path: Option<PathBuf>, factory: bool) -> KitId {
        let id = KitId(self.next_id);
        self.next_id += 1;
        self.kits.push(KitSummary {
            id,
            name: name.to_owned(),
            path,
            dirty: false,
            factory,
        });
        id
    }

    /// Every kit, sorted by name.
    pub fn kits(&self) -> Vec<&KitSummary> {
        let mut kits: Vec<_> = self.kits.iter().collect();
        kits.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        kits
    }

    /// Kits matching the current search text.
    pub fn visible_kits(&self) -> Vec<&KitSummary> {
        let needle = self.search.to_lowercase();
        self.kits()
            .into_iter()
            .filter(|kit| needle.is_empty() || kit.name.to_lowercase().contains(&needle))
            .collect()
    }

    /// Number of kits in the catalogue.
    pub fn len(&self) -> usize {
        self.kits.len()
    }

    /// Whether the catalogue is empty.
    pub fn is_empty(&self) -> bool {
        self.kits.is_empty()
    }

    /// The current search text.
    pub fn search(&self) -> &str {
        &self.search
    }

    /// Set the search text.
    pub fn set_search(&mut self, search: impl Into<String>) {
        self.search = search.into();
    }

    /// The active kit, if any.
    pub fn active(&self) -> Option<KitId> {
        self.active
    }

    /// Look up a kit.
    pub fn get(&self, id: KitId) -> Option<&KitSummary> {
        self.kits.iter().find(|kit| kit.id == id)
    }

    /// Rename the active kit without marking it dirty.
    ///
    /// Used when a kit is projected from the model layer, where the on-disk
    /// name is the truth and no save is pending.
    pub fn set_active_name(&mut self, name: impl Into<String>) {
        let name = name.into();
        if let Some(active) = self.active
            && let Some(kit) = self.kits.iter_mut().find(|kit| kit.id == active)
        {
            kit.name = name;
        }
    }

    /// Whether the active kit has unsaved changes.
    pub fn has_unsaved_changes(&self) -> bool {
        self.active
            .and_then(|id| self.get(id))
            .is_some_and(|kit| kit.dirty)
    }

    /// Mark the active kit dirty or clean.
    pub fn set_dirty(&mut self, dirty: bool) {
        if let Some(id) = self.active
            && let Some(kit) = self.kits.iter_mut().find(|kit| kit.id == id)
        {
            kit.dirty = dirty;
        }
    }

    /// The load state.
    pub fn load_status(&self) -> &LoadStatus {
        &self.load
    }

    /// The pending unsaved-changes decision.
    pub fn pending(&self) -> Option<PendingAction> {
        self.pending
    }

    /// Create a kit and persist it. The new kit does not become active.
    pub fn create(&mut self, name: impl Into<String>) -> Result<KitRequest, KitError> {
        let name = name.into();
        self.check_name(&name, None)?;
        let id = self.insert(&name, None, false);
        Ok(KitRequest::Create(id))
    }

    /// Duplicate a kit. The copy is named `<name> copy`, `<name> copy 2`, ...
    pub fn duplicate(&mut self, source: KitId) -> Result<KitRequest, KitError> {
        let source = self.get(source).ok_or(KitError::UnknownKit(source))?;
        let base = format!("{} copy", source.name);
        let mut candidate = base.clone();
        let mut counter = 2;
        while self.kits.iter().any(|kit| kit.name == candidate) {
            candidate = format!("{base} {counter}");
            counter += 1;
        }
        let id = self.insert(&candidate, None, false);
        Ok(KitRequest::Create(id))
    }

    /// Rename a kit, rejecting empty or duplicate names.
    pub fn rename(&mut self, id: KitId, name: impl Into<String>) -> Result<KitRequest, KitError> {
        let name = name.into();
        self.check_name(&name, Some(id))?;
        let kit = self
            .kits
            .iter_mut()
            .find(|kit| kit.id == id)
            .ok_or(KitError::UnknownKit(id))?;
        kit.name = name;
        kit.dirty = true;
        Ok(KitRequest::Rename(id))
    }

    /// Delete a kit. When the active kit is deleted, a safe fallback becomes
    /// active. The last kit cannot be deleted.
    pub fn delete(&mut self, id: KitId) -> Result<KitRequest, KitError> {
        let kit = self.get(id).ok_or(KitError::UnknownKit(id))?;
        if kit.factory {
            return Err(KitError::FactoryReadOnly(id));
        }
        if self.kits.len() <= 1 {
            return Err(KitError::LastKit);
        }

        let index = self.kits.iter().position(|kit| kit.id == id).expect("exists");
        self.kits.remove(index);
        if self.active == Some(id) {
            // Prefer the next kit, otherwise the previous one. Either keeps the
            // app in a valid state without opening a file picker.
            let fallback = self
                .kits
                .get(index.min(self.kits.len().saturating_sub(1)))
                .map(|kit| kit.id);
            self.active = fallback;
            self.load = match fallback {
                Some(kit) => LoadStatus::Ready { kit },
                None => LoadStatus::Idle,
            };
        }
        Ok(KitRequest::Delete(id))
    }

    /// Ask to switch kits. When the active kit has unsaved changes the request
    /// is parked and [`Self::pending`] reports it; the caller must show a
    /// prompt and call [`Self::confirm_pending`] or [`Self::cancel_pending`].
    pub fn request_switch(&mut self, id: KitId) -> Result<Option<KitRequest>, KitError> {
        if self.get(id).is_none() {
            return Err(KitError::UnknownKit(id));
        }
        if self.load.is_loading() {
            return Err(KitError::Busy);
        }
        if self.active == Some(id) {
            return Ok(None);
        }
        if self.has_unsaved_changes() {
            self.pending = Some(PendingAction::Switch(id));
            return Ok(None);
        }
        Ok(Some(self.start_load(id)))
    }

    /// Ask to delete a kit, guarding the active kit's unsaved changes.
    pub fn request_delete(&mut self, id: KitId) -> Result<Option<KitRequest>, KitError> {
        let is_active = self.active == Some(id);
        if is_active && self.has_unsaved_changes() {
            self.pending = Some(PendingAction::Delete(id));
            return Ok(None);
        }
        self.delete(id).map(Some)
    }

    /// Confirm the parked action.
    pub fn confirm_pending(&mut self) -> Result<Option<KitRequest>, KitError> {
        match self.pending.take() {
            Some(PendingAction::Switch(id)) => {
                self.set_dirty(false);
                Ok(Some(self.start_load(id)))
            }
            Some(PendingAction::Delete(id)) => self.delete(id).map(Some),
            None => Ok(None),
        }
    }

    /// Discard the parked action and keep the current kit.
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }

    /// Start loading a kit.
    pub fn start_load(&mut self, id: KitId) -> KitRequest {
        self.load = LoadStatus::Loading { kit: id, percent: 0 };
        KitRequest::Load(id)
    }

    /// Publish loader progress.
    pub fn set_load_progress(&mut self, percent: u8) {
        if let LoadStatus::Loading { kit, .. } = self.load {
            self.load = LoadStatus::Loading {
                kit,
                percent: percent.min(100),
            };
        }
    }

    /// Complete a load.
    pub fn finish_load(&mut self, id: KitId, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.active = Some(id);
                if let Some(kit) = self.kits.iter_mut().find(|kit| kit.id == id) {
                    kit.dirty = false;
                }
                self.load = LoadStatus::Ready { kit: id };
            }
            Err(message) => {
                self.load = LoadStatus::Failed { kit: id, message };
            }
        }
    }

    /// Retry the failed load. Returns the request to issue.
    pub fn retry(&mut self) -> Option<KitRequest> {
        match &self.load {
            LoadStatus::Failed { kit, .. } => Some(self.start_load(*kit)),
            _ => None,
        }
    }

    /// Dismiss a failure, leaving the previously active kit in place.
    pub fn dismiss_failure(&mut self) {
        if matches!(self.load, LoadStatus::Failed { .. }) {
            self.load = match self.active {
                Some(kit) => LoadStatus::Ready { kit },
                None => LoadStatus::Idle,
            };
        }
    }

    /// Cycle to the next kit in set order, wrapping.
    pub fn next_kit(&mut self) -> Result<Option<KitRequest>, KitError> {
        self.step_kit(1)
    }

    /// Cycle to the previous kit in set order, wrapping.
    pub fn previous_kit(&mut self) -> Result<Option<KitRequest>, KitError> {
        self.step_kit(-1)
    }

    fn step_kit(&mut self, direction: isize) -> Result<Option<KitRequest>, KitError> {
        if self.kits.is_empty() {
            return Ok(None);
        }
        let current = self
            .active
            .and_then(|id| self.kits.iter().position(|kit| kit.id == id))
            .unwrap_or(0);
        let count = self.kits.len() as isize;
        let next = (current as isize + direction).rem_euclid(count) as usize;
        let id = self.kits[next].id;
        self.request_switch(id)
    }

    fn check_name(&self, name: &str, ignore: Option<KitId>) -> Result<(), KitError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(KitError::EmptyName);
        }
        if self
            .kits
            .iter()
            .any(|kit| Some(kit.id) != ignore && kit.name.eq_ignore_ascii_case(trimmed))
        {
            return Err(KitError::DuplicateName(trimmed.to_owned()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factory_kit_is_active_on_first_launch() {
        let browser = KitBrowser::with_factory_kit();
        let active = browser.active().expect("active");
        assert_eq!(browser.get(active).expect("kit").name, "Default Kit");
        assert_eq!(
            browser.load_status(),
            &LoadStatus::Ready { kit: active }
        );
        assert_eq!(browser.kits().len(), 1);
    }

    #[test]
    fn create_rejects_empty_and_duplicate_names() {
        let mut browser = KitBrowser::with_factory_kit();
        assert_eq!(browser.create("  "), Err(KitError::EmptyName));
        assert!(matches!(
            browser.create("default kit"),
            Err(KitError::DuplicateName(_))
        ));
        let request = browser.create("Rock").expect("create");
        assert!(matches!(request, KitRequest::Create(_)));
        assert_eq!(browser.len(), 2);
    }

    #[test]
    fn duplicate_increments_the_copy_suffix() {
        let mut browser = KitBrowser::with_factory_kit();
        let active = browser.active().expect("active");
        browser.duplicate(active).expect("first copy");
        browser.duplicate(active).expect("second copy");
        let names: Vec<_> = browser.kits().iter().map(|kit| kit.name.clone()).collect();
        assert!(names.contains(&"Default Kit copy".to_owned()));
        assert!(names.contains(&"Default Kit copy 2".to_owned()));
    }

    #[test]
    fn rename_updates_the_row_and_marks_it_dirty() {
        let mut browser = KitBrowser::with_factory_kit();
        let active = browser.active().expect("active");
        browser.rename(active, "Live").expect("rename");
        assert_eq!(browser.get(active).expect("kit").name, "Live");
        assert!(browser.has_unsaved_changes());
    }

    #[test]
    fn rename_to_the_same_name_is_allowed() {
        let mut browser = KitBrowser::with_factory_kit();
        let active = browser.active().expect("active");
        assert!(browser.rename(active, "Default Kit").is_ok());
    }

    #[test]
    fn deleting_the_active_kit_selects_a_safe_fallback() {
        let mut browser = KitBrowser::with_factory_kit();
        let first = browser.active().expect("active");
        let second = match browser.create("Two").expect("create") {
            KitRequest::Create(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        browser.request_switch(second).expect("switch");
        browser.finish_load(second, Ok(()));
        assert_eq!(browser.active(), Some(second));

        browser.delete(second).expect("delete");
        assert_eq!(browser.active(), Some(first));
        assert!(matches!(browser.load_status(), LoadStatus::Ready { .. }));
    }

    #[test]
    fn the_last_kit_cannot_be_deleted() {
        let mut browser = KitBrowser::with_factory_kit();
        let active = browser.active().expect("active");
        assert_eq!(browser.delete(active), Err(KitError::FactoryReadOnly(active)));

        let mut browser = KitBrowser::new();
        let only = match browser.create("Only").expect("create") {
            KitRequest::Create(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(browser.delete(only), Err(KitError::LastKit));
    }

    #[test]
    fn factory_kits_are_read_only() {
        let browser = KitBrowser::with_factory_kit();
        let active = browser.active().expect("active");
        assert_eq!(
            browser.kits().iter().find(|kit| kit.id == active).map(|kit| kit.factory),
            Some(true)
        );
    }

    #[test]
    fn switching_with_unsaved_changes_parks_a_pending_action() {
        let mut browser = KitBrowser::with_factory_kit();
        let first = browser.active().expect("active");
        let second = match browser.create("Two").expect("create") {
            KitRequest::Create(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        browser.set_dirty(true);
        assert_eq!(browser.request_switch(second).expect("request"), None);
        assert_eq!(browser.pending(), Some(PendingAction::Switch(second)));
        assert_eq!(browser.active(), Some(first), "the old kit stays active until confirmed");

        let request = browser.confirm_pending().expect("confirm").expect("request");
        assert_eq!(request, KitRequest::Load(second));
        assert!(!browser.has_unsaved_changes());
    }

    #[test]
    fn cancelling_a_pending_switch_keeps_the_current_kit() {
        let mut browser = KitBrowser::with_factory_kit();
        let first = browser.active().expect("active");
        let second = match browser.create("Two").expect("create") {
            KitRequest::Create(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        browser.set_dirty(true);
        browser.request_switch(second).expect("request");
        browser.cancel_pending();
        assert_eq!(browser.pending(), None);
        assert_eq!(browser.active(), Some(first));
    }

    #[test]
    fn a_load_is_asynchronous_and_reports_progress() {
        let mut browser = KitBrowser::with_factory_kit();
        let second = match browser.create("Two").expect("create") {
            KitRequest::Create(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        let request = browser.request_switch(second).expect("request").expect("load");
        assert_eq!(request, KitRequest::Load(second));
        assert!(browser.load_status().is_loading());

        browser.set_load_progress(40);
        assert_eq!(
            browser.load_status(),
            &LoadStatus::Loading {
                kit: second,
                percent: 40
            }
        );
        assert_eq!(
            browser.request_switch(browser.active().expect("active")),
            Err(KitError::Busy),
            "the UI must not start a second load while one is in flight"
        );

        browser.finish_load(second, Ok(()));
        assert_eq!(browser.active(), Some(second));
        assert_eq!(browser.load_status(), &LoadStatus::Ready { kit: second });
    }

    #[test]
    fn a_failed_load_is_visible_and_retryable() {
        let mut browser = KitBrowser::with_factory_kit();
        let first = browser.active().expect("active");
        let second = match browser.create("Two").expect("create") {
            KitRequest::Create(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        browser.request_switch(second).expect("request");
        browser.finish_load(second, Err("sample kick.wav is missing".to_owned()));
        assert_eq!(
            browser.load_status(),
            &LoadStatus::Failed {
                kit: second,
                message: "sample kick.wav is missing".to_owned()
            }
        );
        assert_eq!(browser.active(), Some(first), "a failed load leaves the old kit intact");

        let retry = browser.retry().expect("retry");
        assert_eq!(retry, KitRequest::Load(second));
        assert!(browser.load_status().is_loading());
        browser.finish_load(second, Err("still missing".to_owned()));
        browser.dismiss_failure();
        assert_eq!(browser.load_status(), &LoadStatus::Ready { kit: first });
    }

    #[test]
    fn search_filters_case_insensitively() {
        let mut browser = KitBrowser::with_factory_kit();
        browser.create("Rock").expect("create");
        browser.create("Jazz").expect("create");
        browser.set_search("ro");
        let names: Vec<_> = browser
            .visible_kits()
            .iter()
            .map(|kit| kit.name.clone())
            .collect();
        assert_eq!(names, vec!["Rock".to_owned()]);
        browser.set_search("");
        assert_eq!(browser.visible_kits().len(), 3);
    }

    #[test]
    fn next_and_previous_wrap_the_catalogue() {
        let mut browser = KitBrowser::with_factory_kit();
        let first = browser.active().expect("active");
        let second = match browser.create("Two").expect("create") {
            KitRequest::Create(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        browser.request_switch(second).expect("switch");
        browser.finish_load(second, Ok(()));

        let request = browser.next_kit().expect("next").expect("request");
        assert_eq!(request, KitRequest::Load(first));
        browser.finish_load(first, Ok(()));
        let request = browser.previous_kit().expect("previous").expect("request");
        assert_eq!(request, KitRequest::Load(second));
    }

    #[test]
    fn unknown_kits_are_rejected() {
        let mut browser = KitBrowser::with_factory_kit();
        let ghost = KitId(999);
        assert_eq!(browser.request_switch(ghost), Err(KitError::UnknownKit(ghost)));
    }
}
