//! Application-level kit workspace state.
//!
//! This is a headless state machine for the current kit, pad selection, dirty
//! tracking, and asynchronous load status. The app crate can mirror it into UI
//! widgets without reimplementing transition rules.

use thiserror::Error;

use super::model::{Kit, KitId, PadId};

/// Lifecycle of an asynchronous kit load.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum LoadStatus {
    /// Nothing has been requested.
    #[default]
    Idle,
    /// A load is in flight.
    Loading {
        /// Kit being loaded.
        kit_id: KitId,
    },
    /// The kit is loaded and ready.
    Loaded {
        /// Kit that is loaded.
        kit_id: KitId,
    },
    /// The load failed.
    Failed {
        /// Kit that failed.
        kit_id: KitId,
        /// Human-readable reason.
        message: String,
    },
}

impl LoadStatus {
    /// Kit associated with this status, if any.
    pub fn kit_id(&self) -> Option<KitId> {
        match self {
            Self::Idle => None,
            Self::Loading { kit_id } | Self::Loaded { kit_id } | Self::Failed { kit_id, .. } => {
                Some(*kit_id)
            }
        }
    }

    /// True while a load is in flight.
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }
}

/// Invalid state transitions.
#[derive(Clone, PartialEq, Eq, Debug, Error)]
pub enum StateError {
    /// A completion arrived for a kit that was not loading.
    #[error("no load in progress for kit {0}")]
    NoLoadInProgress(KitId),
    /// A completion arrived for a different kit.
    #[error("load in progress for {expected}, got completion for {actual}")]
    WrongKit {
        /// Kit currently loading.
        expected: KitId,
        /// Kit in the completion event.
        actual: KitId,
    },
}

/// Current kit workspace state.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KitWorkspaceState {
    active_kit: Option<KitId>,
    selected_pad: Option<PadId>,
    dirty: bool,
    status: LoadStatus,
    revision: u64,
    last_error: Option<String>,
}

impl Default for KitWorkspaceState {
    fn default() -> Self {
        Self {
            active_kit: None,
            selected_pad: None,
            dirty: false,
            status: LoadStatus::Idle,
            revision: 0,
            last_error: None,
        }
    }
}

impl KitWorkspaceState {
    /// Fresh state with no kit selected.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current active kit id.
    pub fn active_kit(&self) -> Option<KitId> {
        self.active_kit
    }

    /// Current selected pad.
    pub fn selected_pad(&self) -> Option<PadId> {
        self.selected_pad
    }

    /// True when there are unsaved edits.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Current load status.
    pub fn status(&self) -> &LoadStatus {
        &self.status
    }

    /// Monotonic edit/save revision, useful for cache invalidation.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Last failure message, if any.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Begin loading a kit.
    pub fn begin_load(&mut self, kit_id: KitId) {
        self.status = LoadStatus::Loading { kit_id };
        self.last_error = None;
        self.selected_pad = None;
        self.dirty = false;
    }

    /// Complete a load successfully.
    pub fn finish_load(&mut self, kit_id: KitId, kit: &Kit) -> Result<(), StateError> {
        match &self.status {
            LoadStatus::Loading { kit_id: loading } if *loading == kit_id => {
                self.status = LoadStatus::Loaded { kit_id };
                self.active_kit = Some(kit_id);
                self.selected_pad = kit.pads.first().map(|pad| pad.id);
                self.dirty = false;
                self.last_error = None;
                self.revision += 1;
                Ok(())
            }
            LoadStatus::Loading { kit_id: loading } => Err(StateError::WrongKit {
                expected: *loading,
                actual: kit_id,
            }),
            _ => Err(StateError::NoLoadInProgress(kit_id)),
        }
    }

    /// Complete a load with an error.
    pub fn fail_load(
        &mut self,
        kit_id: KitId,
        message: impl Into<String>,
    ) -> Result<(), StateError> {
        let message = message.into();
        match &self.status {
            LoadStatus::Loading { kit_id: loading } if *loading == kit_id => {
                self.status = LoadStatus::Failed {
                    kit_id,
                    message: message.clone(),
                };
                self.last_error = Some(message);
                self.dirty = false;
                Ok(())
            }
            LoadStatus::Loading { kit_id: loading } => Err(StateError::WrongKit {
                expected: *loading,
                actual: kit_id,
            }),
            _ => Err(StateError::NoLoadInProgress(kit_id)),
        }
    }

    /// Select a pad (or clear the selection).
    pub fn select_pad(&mut self, pad: Option<PadId>) {
        self.selected_pad = pad;
    }

    /// Record an edit, bumping the revision.
    pub fn mark_dirty(&mut self) -> u64 {
        self.dirty = true;
        self.revision += 1;
        self.revision
    }

    /// Record a successful save.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
        self.revision += 1;
    }

    /// True when the state is loaded and not busy.
    pub fn is_ready(&self) -> bool {
        matches!(self.status, LoadStatus::Loaded { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::Kit;

    #[test]
    fn load_transitions_are_checked() {
        let kit = Kit::silent_default();
        let id = kit.id;
        let mut state = KitWorkspaceState::new();
        assert!(state.finish_load(id, &kit).is_err());

        state.begin_load(id);
        assert!(state.status().is_loading());
        assert!(state.finish_load(KitId::new(), &kit).is_err());
        state.finish_load(id, &kit).unwrap();
        assert!(state.is_ready());
        assert_eq!(state.active_kit(), Some(id));
        assert_eq!(state.selected_pad(), Some(kit.pads[0].id));
    }

    #[test]
    fn failed_load_records_error() {
        let id = KitId::new();
        let mut state = KitWorkspaceState::new();
        state.begin_load(id);
        state.fail_load(id, "missing file").unwrap();
        assert!(matches!(state.status(), LoadStatus::Failed { .. }));
        assert_eq!(state.last_error(), Some("missing file"));
        assert!(!state.is_ready());
    }

    #[test]
    fn dirty_and_saved_revisions_monotonic() {
        let mut state = KitWorkspaceState::new();
        let first = state.mark_dirty();
        let second = state.mark_dirty();
        assert!(second > first);
        assert!(state.is_dirty());
        state.mark_saved();
        assert!(!state.is_dirty());
        assert!(state.revision() > second);
    }

    #[test]
    fn changing_kit_clears_selection_and_dirty() {
        let kit = Kit::silent_default();
        let mut state = KitWorkspaceState::new();
        state.begin_load(kit.id);
        state.finish_load(kit.id, &kit).unwrap();
        state.select_pad(Some(kit.pads[3].id));
        state.mark_dirty();
        let next = KitId::new();
        state.begin_load(next);
        assert_eq!(state.selected_pad(), None);
        assert!(!state.is_dirty());
    }
}
