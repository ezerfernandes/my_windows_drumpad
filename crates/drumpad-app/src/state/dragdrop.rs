//! Drag-and-drop assignment: classification, preview, confirmation, and undo.
//!
//! Windows hands the shell a list of paths. A path is either an audio file we
//! can assign, a folder for the batch importer, or something unsupported. Only
//! the first category can change a pad, and only after [`DropController::confirm`]
//! — which is what makes "accidental drops can be undone" true even for a drop
//! that never showed the confirmation.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use super::pad::{PadId, Pads};

/// Extensions the app can decode. The samples crate owns the decoder; this list
/// is only the UI's cheap pre-check so an obvious `.txt` never opens a decoder.
pub const AUDIO_EXTENSIONS: [&str; 6] = ["wav", "aif", "aiff", "flac", "mp3", "ogg"];

/// Whether a path looks like a supported audio file.
pub fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .is_some_and(|extension| AUDIO_EXTENSIONS.contains(&extension.as_str()))
}

/// A classified drop.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DropSummary {
    /// Supported audio files, in the order Windows supplied them.
    pub audio: Vec<PathBuf>,
    /// Folders handed to the batch importer.
    pub folders: Vec<PathBuf>,
    /// Files with no supported decoder.
    pub unsupported: Vec<PathBuf>,
}

impl DropSummary {
    /// Whether the drop can change any pad.
    pub fn changes_pads(&self) -> bool {
        !self.audio.is_empty()
    }

    /// One-line status for the toast.
    pub fn message(&self) -> String {
        if self.audio.is_empty() && self.folders.is_empty() {
            return format!(
                "Nothing imported: {} unsupported file(s)",
                self.unsupported.len()
            );
        }
        let mut parts = Vec::new();
        if !self.audio.is_empty() {
            parts.push(format!("{} audio file(s)", self.audio.len()));
        }
        if !self.folders.is_empty() {
            parts.push(format!("{} folder(s)", self.folders.len()));
        }
        if !self.unsupported.is_empty() {
            parts.push(format!("{} skipped", self.unsupported.len()));
        }
        parts.join(", ")
    }
}

/// Classify dropped paths, de-duplicating repeated paths.
pub fn classify(paths: &[PathBuf]) -> DropSummary {
    let mut seen = HashSet::new();
    let mut summary = DropSummary::default();
    for path in paths {
        if !seen.insert(path.clone()) {
            continue;
        }
        if path.is_dir() {
            summary.folders.push(path.clone());
        } else if is_supported_audio(path) {
            summary.audio.push(path.clone());
        } else {
            summary.unsupported.push(path.clone());
        }
    }
    summary
}

/// A proposed assignment waiting for confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropPreview {
    /// The classified drop.
    pub summary: DropSummary,
    /// Pad and file pairs, in assignment order.
    pub targets: Vec<(PadId, PathBuf)>,
}

impl DropPreview {
    /// Pads that would change.
    pub fn pads(&self) -> impl Iterator<Item = PadId> + '_ {
        self.targets.iter().map(|(pad, _)| *pad)
    }
}

/// How a drop failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropError {
    /// Every dropped path was unsupported.
    NoSupportedFiles,
    /// There are no free pads for a multi-file drop.
    NoEmptyPads,
}

impl fmt::Display for DropError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSupportedFiles => {
                formatter.write_str("no supported audio files in this drop")
            }
            Self::NoEmptyPads => formatter.write_str("there are no empty pads left to assign"),
        }
    }
}

impl std::error::Error for DropError {}

/// A completed assignment that can be undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropUndo {
    /// Previous sample names, `None` for a pad that was empty.
    pub previous: Vec<(PadId, Option<String>)>,
    /// Pads the drop wrote.
    pub assigned: Vec<PadId>,
}

/// Drop lifecycle: idle → preview → confirmed | cancelled, with one undo slot.
#[derive(Debug, Clone, Default)]
pub struct DropController {
    preview: Option<DropPreview>,
    undo: Option<DropUndo>,
}

impl DropController {
    /// Empty controller.
    pub fn new() -> Self {
        Self::default()
    }

    /// The pending preview.
    pub fn preview(&self) -> Option<&DropPreview> {
        self.preview.as_ref()
    }

    /// Whether a drop is waiting for confirmation.
    pub fn is_confirming(&self) -> bool {
        self.preview.is_some()
    }

    /// The last assignment that can be undone.
    pub fn undo(&self) -> Option<&DropUndo> {
        self.undo.as_ref()
    }

    /// Classify a drop and build a preview. Nothing is assigned yet.
    ///
    /// `target` is the pad under the cursor, when the shell can determine one.
    /// The remaining files fill the selection and then the empty pads in grid
    /// order, so a multi-file drop is a single confirmable action.
    pub fn begin(
        &mut self,
        paths: &[PathBuf],
        pads: &Pads,
        target: Option<PadId>,
    ) -> Result<&DropPreview, DropError> {
        let summary = classify(paths);
        if !summary.changes_pads() {
            self.preview = None;
            return Err(DropError::NoSupportedFiles);
        }

        // The explicit target (or the selected pad) may be overwritten. Every
        // further file only fills pads that are still empty, in grid order, so a
        // batch drop never silently destroys a second sample.
        let first_pad = target.unwrap_or_else(|| pads.selected());
        let mut targets = vec![(first_pad, summary.audio[0].clone())];

        let mut empty: Vec<PadId> = PadId::ALL
            .into_iter()
            .filter(|pad| *pad != first_pad && !pads.get(*pad).assigned)
            .collect();
        empty.reverse(); // pop() from the front of grid order

        for path in summary.audio.iter().skip(1) {
            let Some(pad) = empty.pop() else {
                self.preview = None;
                return Err(DropError::NoEmptyPads);
            };
            targets.push((pad, path.clone()));
        }

        self.preview = Some(DropPreview { summary, targets });
        Ok(self.preview.as_ref().expect("just set"))
    }

    /// Assign the previewed files. Returns the undo record.
    pub fn confirm(&mut self, pads: &mut Pads) -> Option<DropUndo> {
        let preview = self.preview.take()?;
        let mut previous = Vec::with_capacity(preview.targets.len());
        for (pad, path) in &preview.targets {
            previous.push((*pad, pads.get(*pad).sample.clone()));
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            pads.set_sample(*pad, Some(name));
        }
        let undo = DropUndo {
            previous,
            assigned: preview.targets.iter().map(|(pad, _)| *pad).collect(),
        };
        self.undo = Some(undo.clone());
        Some(undo)
    }

    /// Discard the preview without touching a pad.
    pub fn cancel(&mut self) {
        self.preview = None;
    }

    /// Revert the last confirmed drop. Returns the pads restored.
    pub fn undo_assignment(&mut self, pads: &mut Pads) -> Option<Vec<PadId>> {
        let undo = self.undo.take()?;
        for (pad, previous) in &undo.previous {
            pads.set_sample(*pad, previous.clone());
        }
        Some(undo.assigned)
    }

    /// Whether an undo is available.
    pub fn can_undo(&self) -> bool {
        self.undo.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn supported_extensions_are_recognised_case_insensitively() {
        for name in ["kick.wav", "kick.WAV", "hit.Aif", "snare.flac", "loop.mp3", "vox.ogg"] {
            assert!(is_supported_audio(Path::new(name)), "{name}");
        }
        for name in ["notes.txt", "kick.wav.bak", "image.png", "noext"] {
            assert!(!is_supported_audio(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn classification_keeps_unsupported_files_out_of_the_audio_list() {
        let summary = classify(&paths(&["kick.wav", "notes.txt", "snare.ogg"]));
        assert_eq!(summary.audio, paths(&["kick.wav", "snare.ogg"]));
        assert_eq!(summary.unsupported, paths(&["notes.txt"]));
        assert!(summary.changes_pads());
        assert!(summary.message().contains("2 audio"));
        assert!(summary.message().contains("1 skipped"));
    }

    #[test]
    fn duplicate_paths_are_collapsed() {
        let summary = classify(&paths(&["kick.wav", "kick.wav", "snare.wav"]));
        assert_eq!(summary.audio.len(), 2);
    }

    #[test]
    fn an_unsupported_only_drop_cannot_change_a_pad() {
        let mut controller = DropController::new();
        let mut pads = Pads::new();
        let error = controller
            .begin(&paths(&["notes.txt"]), &pads, Some(PadId::new(0)))
            .expect_err("unsupported");
        assert_eq!(error, DropError::NoSupportedFiles);
        assert!(controller.preview().is_none());
        assert!(!controller.can_undo());

        controller.confirm(&mut pads);
        assert_eq!(pads.get(PadId::new(0)).sample, None, "pads must not change");
    }

    #[test]
    fn a_single_file_dropped_on_a_pad_targets_that_pad() {
        let mut controller = DropController::new();
        let pads = Pads::new();
        let preview = controller
            .begin(&paths(&["kick.wav"]), &pads, Some(PadId::new(7)))
            .expect("preview")
            .clone();
        assert_eq!(preview.targets, vec![(PadId::new(7), PathBuf::from("kick.wav"))]);
    }

    #[test]
    fn confirming_assigns_the_sample_name() {
        let mut controller = DropController::new();
        let mut pads = Pads::new();
        controller
            .begin(&paths(&["D:\\Samples\\kick.wav"]), &pads, Some(PadId::new(3)))
            .expect("preview");
        let undo = controller.confirm(&mut pads).expect("confirm");
        assert_eq!(pads.get(PadId::new(3)).sample.as_deref(), Some("kick.wav"));
        assert!(pads.get(PadId::new(3)).assigned);
        assert_eq!(undo.assigned, vec![PadId::new(3)]);
    }

    #[test]
    fn an_accidental_drop_can_be_undone() {
        let mut controller = DropController::new();
        let mut pads = Pads::new();
        pads.set_sample(PadId::new(0), Some("old.wav".to_owned()));

        controller
            .begin(&paths(&["new.wav"]), &pads, Some(PadId::new(0)))
            .expect("preview");
        controller.confirm(&mut pads);
        assert_eq!(pads.get(PadId::new(0)).sample.as_deref(), Some("new.wav"));

        let restored = controller.undo_assignment(&mut pads).expect("undo");
        assert_eq!(restored, vec![PadId::new(0)]);
        assert_eq!(pads.get(PadId::new(0)).sample.as_deref(), Some("old.wav"));
        assert!(!controller.can_undo());
    }

    #[test]
    fn cancelling_a_preview_leaves_every_pad_untouched() {
        let mut controller = DropController::new();
        let mut pads = Pads::new();
        controller
            .begin(&paths(&["kick.wav"]), &pads, Some(PadId::new(2)))
            .expect("preview");
        controller.cancel();
        controller.confirm(&mut pads);
        assert!(pads.iter().all(|pad| pad.sample.is_none()));
    }

    #[test]
    fn a_batch_drop_fills_the_selected_pad_then_empty_pads() {
        let mut controller = DropController::new();
        let mut pads = Pads::new();
        pads.select(PadId::new(4));
        let preview = controller
            .begin(
                &paths(&["a.wav", "b.wav", "c.wav"]),
                &pads,
                None,
            )
            .expect("preview")
            .clone();
        assert_eq!(
            preview.pads().collect::<Vec<_>>(),
            vec![PadId::new(4), PadId::new(0), PadId::new(1)]
        );
    }

    #[test]
    fn a_batch_larger_than_the_free_pads_is_refused() {
        let mut controller = DropController::new();
        let mut pads = Pads::new();
        for id in PadId::ALL {
            pads.set_sample(id, Some("used.wav".to_owned()));
        }
        let names: Vec<String> = (0..3).map(|index| format!("{index}.wav")).collect();
        let dropped: Vec<PathBuf> = names.iter().map(PathBuf::from).collect();
        let error = controller
            .begin(&dropped, &pads, Some(PadId::new(0)))
            .expect_err("no room");
        assert_eq!(error, DropError::NoEmptyPads);
    }

    #[test]
    fn folders_are_reported_for_the_batch_importer_but_not_assigned() {
        let temp = tempfile::tempdir().expect("temp");
        let summary = classify(&[temp.path().to_path_buf()]);
        assert_eq!(summary.folders.len(), 1);
        assert!(summary.audio.is_empty());
        assert!(!summary.changes_pads());
        assert!(summary.message().contains("1 folder"));
    }
}
