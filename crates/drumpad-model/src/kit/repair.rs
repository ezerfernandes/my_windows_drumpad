//! Explicit, best-effort kit repair.
//!
//! Repair never guesses about missing audio. It fixes structural problems that
//! have an unambiguous answer (wrong pad count, duplicate ids, inverted
//! velocity ranges, out-of-range floats, duplicate mappings) and leaves
//! anything that needs a sample decode or a human decision in the remaining
//! validation report. Every change is recorded and logged.

use std::collections::BTreeSet;

use super::model::Kit;
use super::model::{
    CURRENT_FORMAT_VERSION, DEFAULT_PAD_MIDI_NOTES, DEFAULT_PAD_SCANCODES, KeyMapping,
    MAX_LAYERS_PER_PAD, MidiChannel, MidiMapping, MidiNote, PAD_COUNT, Pad, PadId, PhysicalKey,
};
use super::validation::{ValidationReport, validate_kit_report};

/// One repair that was applied.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RepairAction {
    /// Location inside the document.
    pub path: String,
    /// What changed.
    pub description: String,
}

/// Result of repairing a kit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RepairReport {
    /// Applied changes, in document order.
    pub actions: Vec<RepairAction>,
    /// Issues that remain after repair.
    pub remaining: ValidationReport,
}

impl RepairReport {
    /// True when the repair produced a valid kit.
    pub fn is_valid(&self) -> bool {
        self.remaining.is_valid()
    }

    /// True when no changes were necessary.
    pub fn is_clean(&self) -> bool {
        self.actions.is_empty()
    }
}

fn set_field(
    value: &mut f32,
    fallback: f32,
    min: f32,
    max: f32,
    actions: &mut Vec<RepairAction>,
    path: String,
) -> bool {
    if !value.is_finite() || *value < min || *value > max {
        let old = *value;
        *value = if old.is_finite() {
            old.clamp(min, max)
        } else {
            fallback
        };
        actions.push(RepairAction {
            path,
            description: format!("clamped {old} to {}", *value),
        });
        return true;
    }
    false
}

fn truncate_chars(value: &mut String, max: usize) {
    if value.chars().count() > max {
        *value = value.chars().take(max).collect();
    }
}

/// Repair a kit in place and return a report of what changed.
pub fn repair_kit(kit: &mut Kit) -> RepairReport {
    let mut actions = Vec::new();

    if kit.format_version != CURRENT_FORMAT_VERSION {
        let old = kit.format_version;
        kit.format_version = CURRENT_FORMAT_VERSION;
        actions.push(RepairAction {
            path: "format_version".into(),
            description: format!("set {old} to {CURRENT_FORMAT_VERSION}"),
        });
    }

    let original_name = kit.metadata.name.clone();
    kit.metadata.name = kit.metadata.name.trim().to_owned();
    truncate_chars(&mut kit.metadata.name, 128);
    if kit.metadata.name.is_empty() {
        kit.metadata.name = "Untitled Kit".to_owned();
        actions.push(RepairAction {
            path: "metadata.name".into(),
            description: "replaced empty name with `Untitled Kit`".into(),
        });
    } else if kit.metadata.name != original_name {
        actions.push(RepairAction {
            path: "metadata.name".into(),
            description: "trimmed or shortened the kit name".into(),
        });
    }
    if let Some(author) = &mut kit.metadata.author {
        truncate_chars(author, 128);
    }
    if let Some(description) = &mut kit.metadata.description {
        truncate_chars(description, 4096);
    }
    if kit.metadata.tags.len() > 32 {
        kit.metadata.tags.truncate(32);
        actions.push(RepairAction {
            path: "metadata.tags".into(),
            description: "kept the first 32 tags".into(),
        });
    }
    for (index, tag) in kit.metadata.tags.iter_mut().enumerate() {
        let before = tag.clone();
        truncate_chars(tag, 32);
        if *tag != before {
            actions.push(RepairAction {
                path: format!("metadata.tags[{index}]"),
                description: "truncated tag to 32 characters".into(),
            });
        }
    }

    if kit.pads.len() > PAD_COUNT {
        let removed = kit.pads.len() - PAD_COUNT;
        kit.pads.truncate(PAD_COUNT);
        actions.push(RepairAction {
            path: "pads".into(),
            description: format!("removed {removed} pad(s) beyond the 16-pad limit"),
        });
    }
    while kit.pads.len() < PAD_COUNT {
        let slot = kit.pads.len();
        let pad = Pad {
            name: Some(format!("Pad {}", slot + 1)),
            ..Pad::silent(next_unused_pad_id(&kit.pads))
        };
        kit.pads.push(pad);
        actions.push(RepairAction {
            path: format!("pads[{slot}]"),
            description: "added a silent pad to reach 16 pads".into(),
        });
    }

    // Reassign duplicate pad ids deterministically.
    let mut used_pad_ids = BTreeSet::new();
    for slot in 0..kit.pads.len() {
        let id = kit.pads[slot].id;
        if used_pad_ids.insert(id) {
            continue;
        }
        let replacement = next_unused_pad_id_from(&used_pad_ids);
        used_pad_ids.insert(replacement);
        kit.pads[slot].id = replacement;
        actions.push(RepairAction {
            path: format!("pads[{slot}].id"),
            description: format!("reassigned duplicate pad id {id} to {replacement}"),
        });
    }

    for (slot, pad) in kit.pads.iter_mut().enumerate() {
        let base = format!("pads[{slot}]");
        if let Some(name) = &mut pad.name {
            let before = name.clone();
            truncate_chars(name, 64);
            if *name != before {
                actions.push(RepairAction {
                    path: format!("{base}.name"),
                    description: "truncated pad name to 64 characters".into(),
                });
            }
        }
        set_field(
            &mut pad.gain_db,
            0.0,
            -60.0,
            24.0,
            &mut actions,
            format!("{base}.gain_db"),
        );
        set_field(
            &mut pad.pan,
            0.0,
            -1.0,
            1.0,
            &mut actions,
            format!("{base}.pan"),
        );
        set_field(
            &mut pad.tune_semitones,
            0.0,
            -24.0,
            24.0,
            &mut actions,
            format!("{base}.tune_semitones"),
        );

        if pad.layers.len() > MAX_LAYERS_PER_PAD {
            let removed = pad.layers.len() - MAX_LAYERS_PER_PAD;
            pad.layers.truncate(MAX_LAYERS_PER_PAD);
            actions.push(RepairAction {
                path: format!("{base}.layers"),
                description: format!("removed {removed} excess layer(s)"),
            });
        }

        let mut used_layer_ids = BTreeSet::new();
        for index in 0..pad.layers.len() {
            let id = pad.layers[index].id;
            if used_layer_ids.insert(id) {
                continue;
            }
            let mut next = 1u32;
            while used_layer_ids.contains(&super::LayerId::new(next)) {
                next += 1;
            }
            let replacement = super::LayerId::new(next);
            used_layer_ids.insert(replacement);
            pad.layers[index].id = replacement;
            actions.push(RepairAction {
                path: format!("{base}.layers[{index}].id"),
                description: format!("reassigned duplicate layer id {id} to {replacement}"),
            });
        }

        for (index, layer) in pad.layers.iter_mut().enumerate() {
            let layer_base = format!("{base}.layers[{index}]");
            let mut min = layer.velocity.min;
            let mut max = layer.velocity.max;
            let old = (min, max);
            if !(1..=127).contains(&min) {
                min = min.clamp(1, 127);
            }
            if !(1..=127).contains(&max) {
                max = max.clamp(1, 127);
            }
            if min > max {
                std::mem::swap(&mut min, &mut max);
            }
            if (min, max) != old {
                layer.velocity.min = min;
                layer.velocity.max = max;
                actions.push(RepairAction {
                    path: format!("{layer_base}.velocity"),
                    description: format!(
                        "clamped/reordered velocity range from {}..={} to {}..={}",
                        old.0, old.1, min, max
                    ),
                });
            }
            set_field(
                &mut layer.gain_db,
                0.0,
                -60.0,
                24.0,
                &mut actions,
                format!("{layer_base}.gain_db"),
            );
            set_field(
                &mut layer.tune_cents,
                0.0,
                -2400.0,
                2400.0,
                &mut actions,
                format!("{layer_base}.tune_cents"),
            );
            if let Some(loop_points) = layer.loop_points
                && loop_points.end_frame <= loop_points.start_frame
            {
                layer.loop_points = None;
                actions.push(RepairAction {
                    path: format!("{layer_base}.loop_points"),
                    description: "removed an empty or inverted loop range".into(),
                });
            }
        }
    }

    repair_mappings(kit, &mut actions);

    for action in &actions {
        log::warn!(
            target: "drumpad_model::kit::repair",
            "repaired {}: {}",
            action.path,
            action.description
        );
    }

    RepairReport {
        actions,
        remaining: validate_kit_report(kit),
    }
}

fn next_unused_pad_id(pads: &[Pad]) -> PadId {
    (0..=PadId::MAX)
        .map(PadId::from_raw_unchecked)
        .find(|id| !pads.iter().any(|pad| pad.id == *id))
        .unwrap_or_else(|| PadId::from_raw_unchecked(0))
}

fn next_unused_pad_id_from(used: &BTreeSet<PadId>) -> PadId {
    (0..=PadId::MAX)
        .map(PadId::from_raw_unchecked)
        .find(|id| !used.contains(id))
        .unwrap_or_else(|| PadId::from_raw_unchecked(0))
}

fn repair_mappings(kit: &mut Kit, actions: &mut Vec<RepairAction>) {
    let mut used_keys = BTreeSet::new();
    let mut used_midi = BTreeSet::new();

    for slot in 0..kit.pads.len() {
        let base = format!("pads[{slot}]");
        if let Some(mapping) = kit.pads[slot].key
            && !used_keys.insert(mapping.key)
        {
            kit.pads[slot].key = None;
            actions.push(RepairAction {
                path: format!("{base}.key"),
                description: format!(
                    "cleared duplicate keyboard mapping for key {}",
                    mapping.key.raw()
                ),
            });
        }
        if let Some(mapping) = kit.pads[slot].midi
            && !used_midi.insert((mapping.note, mapping.channel))
        {
            kit.pads[slot].midi = None;
            actions.push(RepairAction {
                path: format!("{base}.midi"),
                description: format!(
                    "cleared duplicate MIDI mapping for note {} channel {}",
                    mapping.note.get(),
                    mapping.channel.get()
                ),
            });
        }
    }

    // Fill missing mappings from the default maps without creating conflicts.
    for slot in 0..kit.pads.len() {
        if kit.pads[slot].key.is_none() {
            let candidate = DEFAULT_PAD_SCANCODES
                .get(slot)
                .map(|code| PhysicalKey::from_scancode(*code))
                .or_else(|| {
                    DEFAULT_PAD_SCANCODES
                        .iter()
                        .map(|code| PhysicalKey::from_scancode(*code))
                        .find(|key| !used_keys.contains(key))
                });
            if let Some(key) = candidate
                && used_keys.insert(key)
            {
                kit.pads[slot].key = Some(KeyMapping::new(key));
                actions.push(RepairAction {
                    path: format!("pads[{slot}].key"),
                    description: format!("restored default keyboard mapping (key {})", key.raw()),
                });
            }
        }
        if kit.pads[slot].midi.is_none() {
            let candidate = DEFAULT_PAD_MIDI_NOTES
                .get(slot)
                .map(|note| MidiNote::from_raw_unchecked(*note))
                .or_else(|| {
                    DEFAULT_PAD_MIDI_NOTES
                        .iter()
                        .map(|note| MidiNote::from_raw_unchecked(*note))
                        .find(|note| !used_midi.contains(&(*note, MidiChannel::default())))
                });
            if let Some(note) = candidate
                && used_midi.insert((note, MidiChannel::default()))
            {
                kit.pads[slot].midi = Some(MidiMapping::on_channel_1(note));
                actions.push(RepairAction {
                    path: format!("pads[{slot}].midi"),
                    description: format!("restored default MIDI mapping (note {})", note.get()),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{Kit, Layer, VelocityRange};

    #[test]
    fn clean_kit_needs_no_repairs() {
        let mut kit = Kit::silent_default();
        let report = repair_kit(&mut kit);
        assert!(report.is_clean(), "{:#?}", report.actions);
        assert!(report.is_valid());
    }

    #[test]
    fn structural_damage_is_repaired() {
        let mut kit = Kit::silent_default();
        kit.metadata.name = "   ".into();
        kit.pads.pop();
        kit.pads[1].id = kit.pads[0].id;
        kit.pads[0].pan = 4.0;
        kit.pads[0].gain_db = f32::INFINITY;
        kit.pads[0].layers.push(Layer {
            velocity: VelocityRange { min: 90, max: 10 },
            ..Layer::silent(Default::default())
        });
        kit.pads[2].key = kit.pads[0].key;
        kit.pads[3].key = None;

        let report = repair_kit(&mut kit);
        assert!(report.is_valid(), "{:#?}", report.remaining);
        assert!(!report.is_clean());
        assert_eq!(kit.pads.len(), 16);
        assert_eq!(kit.metadata.name, "Untitled Kit");
        assert_eq!(kit.pads[0].pan, 1.0);
        assert_eq!(kit.pads[0].gain_db, 0.0);
        assert_eq!(kit.pads[0].layers[0].velocity.min, 10);
        assert!(kit.pads[0].layers[0].velocity.max >= 90);
        assert!(kit.validate().is_ok());
    }

    #[test]
    fn unknown_values_are_clamped_and_logged() {
        let mut kit = Kit::silent_default();
        kit.pads[4].gain_db = -100.0;
        kit.pads[4].tune_semitones = 100.0;
        let report = repair_kit(&mut kit);
        assert_eq!(kit.pads[4].gain_db, -60.0);
        assert_eq!(kit.pads[4].tune_semitones, 24.0);
        assert!(
            report
                .actions
                .iter()
                .any(|action| action.path == "pads[4].gain_db")
        );
    }

    #[test]
    fn missing_hash_remains_a_warning() {
        use crate::kit::{ContentHash, RelativeSamplePath, SampleRef};
        let mut kit = Kit::silent_default();
        kit.pads[0].layers.push(Layer::with_sample(
            Default::default(),
            SampleRef {
                path: RelativeSamplePath::new("samples/kick.wav").unwrap(),
                hash: None,
                original_name: None,
            },
        ));
        let report = repair_kit(&mut kit);
        assert!(report.is_valid());
        assert!(report.remaining.warnings().next().is_some());
        // Keep the helper referenced so the import stays intentional.
        let _ = ContentHash::of_bytes(b"x");
    }

    #[test]
    fn repair_normalizes_metadata_and_oversized_collections() {
        let mut kit = Kit::silent_default();
        kit.format_version = 0;
        kit.metadata.name = format!("  {}  ", "n".repeat(140));
        kit.metadata.author = Some("a".repeat(140));
        kit.metadata.description = Some("d".repeat(4_100));
        kit.metadata.tags = (0..34)
            .map(|index| {
                if index == 0 {
                    "t".repeat(40)
                } else {
                    format!("tag-{index}")
                }
            })
            .collect();
        kit.pads[0].name = Some("p".repeat(80));
        kit.pads.push(Pad::silent(PadId::new(15).unwrap()));

        let report = repair_kit(&mut kit);
        assert_eq!(kit.format_version, CURRENT_FORMAT_VERSION);
        assert_eq!(kit.metadata.name.chars().count(), 128);
        assert_eq!(kit.metadata.author.as_ref().unwrap().chars().count(), 128);
        assert_eq!(
            kit.metadata.description.as_ref().unwrap().chars().count(),
            4_096
        );
        assert_eq!(kit.metadata.tags.len(), 32);
        assert_eq!(kit.metadata.tags[0].chars().count(), 32);
        assert_eq!(kit.pads.len(), PAD_COUNT);
        assert_eq!(kit.pads[0].name.as_ref().unwrap().chars().count(), 64);
        for path in [
            "format_version",
            "metadata.name",
            "metadata.tags",
            "metadata.tags[0]",
            "pads",
            "pads[0].name",
        ] {
            assert!(
                report.actions.iter().any(|action| action.path == path),
                "missing repair action for {path}: {:#?}",
                report.actions
            );
        }
        assert!(report.is_valid());
    }

    #[test]
    fn repair_fixes_layers_velocity_loops_and_mapping_conflicts() {
        use crate::kit::{LayerId, SampleLoop};

        let mut kit = Kit::silent_default();
        let mut layers: Vec<Layer> = (0..33)
            .map(|index| Layer::silent(LayerId::new(index as u32 + 1)))
            .collect();
        layers[0].velocity = VelocityRange { min: 0, max: 200 };
        layers[1].id = layers[0].id;
        layers[2].loop_points = Some(SampleLoop {
            start_frame: 10,
            end_frame: 10,
        });
        layers[3].gain_db = -100.0;
        layers[3].tune_cents = 3_000.0;
        kit.pads[0].layers = layers;

        kit.pads[1].key = kit.pads[0].key;
        kit.pads[1].midi = kit.pads[0].midi;
        kit.pads[2].key = None;
        kit.pads[2].midi = None;
        kit.pads[3].key = Some(KeyMapping::new(PhysicalKey::from_scancode(0x04)));
        kit.pads[3].midi = Some(MidiMapping::on_channel_1(MidiNote::new(40).unwrap()));

        let report = repair_kit(&mut kit);
        assert_eq!(kit.pads[0].layers.len(), MAX_LAYERS_PER_PAD);
        assert_eq!(kit.pads[0].layers[0].velocity.min, 1);
        assert_eq!(kit.pads[0].layers[0].velocity.max, 127);
        assert_eq!(kit.pads[0].layers[3].gain_db, -60.0);
        assert_eq!(kit.pads[0].layers[3].tune_cents, 2_400.0);
        assert!(kit.pads[0].layers[2].loop_points.is_none());
        let layer_ids: std::collections::BTreeSet<_> =
            kit.pads[0].layers.iter().map(|layer| layer.id).collect();
        assert_eq!(layer_ids.len(), MAX_LAYERS_PER_PAD);

        for path in [
            "pads[0].layers",
            "pads[0].layers[0].velocity",
            "pads[0].layers[1].id",
            "pads[0].layers[2].loop_points",
            "pads[0].layers[3].gain_db",
            "pads[0].layers[3].tune_cents",
            "pads[1].key",
            "pads[1].midi",
        ] {
            assert!(
                report.actions.iter().any(|action| action.path == path),
                "missing repair action for {path}: {:#?}",
                report.actions
            );
        }
        assert!(kit.pads[1].key.is_some());
        assert!(kit.pads[1].midi.is_some());
        // A missing mapping is left empty when its slot default is already
        // taken; that is a warning, not an invariant violation.
        assert!(kit.pads[2].key.is_none());
        assert!(kit.pads[2].midi.is_none());
        assert!(report.is_valid(), "{:#?}", report.remaining);
        assert!(kit.validate().is_ok());
    }
}
