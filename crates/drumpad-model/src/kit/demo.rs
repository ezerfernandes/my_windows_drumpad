//! Factory demo kit and its licensing manifest.
//!
//! The demo samples are original synthesized one-shots generated for this
//! project and dedicated to CC0. They are embedded in the binary so a portable
//! build always has a kit that makes sound on first run.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::format::{self, FormatError};
use super::model::Kit;

/// The demo kit manifest, embedded at build time.
pub const DEMO_KIT_JSON: &str = include_str!("../../assets/demo-kit/kit.json");
/// Attribution and license text for the demo samples.
pub const DEMO_KIT_LICENSES: &str = include_str!("../../assets/demo-kit/LICENSES.md");
/// File name used when installing the demo kit.
pub const DEMO_MANIFEST: &str = "kit.json";

/// Embedded demo sample files, relative to the kit folder.
pub const DEMO_SAMPLES: &[(&str, &[u8])] = &[
    (
        "samples/kick.wav",
        include_bytes!("../../assets/demo-kit/samples/kick.wav"),
    ),
    (
        "samples/snare.wav",
        include_bytes!("../../assets/demo-kit/samples/snare.wav"),
    ),
    (
        "samples/rim.wav",
        include_bytes!("../../assets/demo-kit/samples/rim.wav"),
    ),
    (
        "samples/clap.wav",
        include_bytes!("../../assets/demo-kit/samples/clap.wav"),
    ),
    (
        "samples/tom_low.wav",
        include_bytes!("../../assets/demo-kit/samples/tom_low.wav"),
    ),
    (
        "samples/tom_mid.wav",
        include_bytes!("../../assets/demo-kit/samples/tom_mid.wav"),
    ),
    (
        "samples/tom_high.wav",
        include_bytes!("../../assets/demo-kit/samples/tom_high.wav"),
    ),
    (
        "samples/floor_tom.wav",
        include_bytes!("../../assets/demo-kit/samples/floor_tom.wav"),
    ),
    (
        "samples/hat_closed.wav",
        include_bytes!("../../assets/demo-kit/samples/hat_closed.wav"),
    ),
    (
        "samples/hat_pedal.wav",
        include_bytes!("../../assets/demo-kit/samples/hat_pedal.wav"),
    ),
    (
        "samples/hat_open.wav",
        include_bytes!("../../assets/demo-kit/samples/hat_open.wav"),
    ),
    (
        "samples/crash.wav",
        include_bytes!("../../assets/demo-kit/samples/crash.wav"),
    ),
    (
        "samples/ride.wav",
        include_bytes!("../../assets/demo-kit/samples/ride.wav"),
    ),
    (
        "samples/ride_bell.wav",
        include_bytes!("../../assets/demo-kit/samples/ride_bell.wav"),
    ),
    (
        "samples/cowbell.wav",
        include_bytes!("../../assets/demo-kit/samples/cowbell.wav"),
    ),
    (
        "samples/tambourine.wav",
        include_bytes!("../../assets/demo-kit/samples/tambourine.wav"),
    ),
];

/// Demo kit loading/install errors.
#[derive(Debug, Error)]
pub enum DemoError {
    /// The destination could not be written.
    #[error("failed to install demo kit at {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The embedded manifest is invalid (a build-time bug).
    #[error("embedded demo kit manifest is invalid: {0}")]
    Format(#[from] FormatError),
    /// A kit already occupies the destination.
    #[error("a kit is already installed at {}", .0.display())]
    AlreadyInstalled(PathBuf),
}

/// Parse the embedded demo kit manifest.
pub fn demo_kit() -> Result<Kit, FormatError> {
    let parsed = format::parse_kit_json(DEMO_KIT_JSON)?;
    Ok(parsed.kit)
}

/// Installed demo sample bytes, for tests and tooling.
pub fn demo_sample_bytes(relative: &str) -> Option<&'static [u8]> {
    DEMO_SAMPLES
        .iter()
        .find(|(path, _)| *path == relative)
        .map(|(_, bytes)| *bytes)
}

/// Write the demo kit and samples into an empty destination folder.
pub fn install_demo_kit(dest: &Path) -> Result<Kit, DemoError> {
    if dest.join(DEMO_MANIFEST).exists() {
        return Err(DemoError::AlreadyInstalled(dest.to_owned()));
    }
    fs::create_dir_all(dest).map_err(|source| DemoError::Io {
        path: dest.to_owned(),
        source,
    })?;
    for (relative, bytes) in DEMO_SAMPLES {
        let target = dest.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| DemoError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        fs::write(&target, bytes).map_err(|source| DemoError::Io {
            path: target,
            source,
        })?;
    }
    fs::write(dest.join(DEMO_MANIFEST), DEMO_KIT_JSON).map_err(|source| DemoError::Io {
        path: dest.join(DEMO_MANIFEST),
        source,
    })?;
    fs::write(dest.join("LICENSES.md"), DEMO_KIT_LICENSES).map_err(|source| DemoError::Io {
        path: dest.join("LICENSES.md"),
        source,
    })?;
    demo_kit().map_err(DemoError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::validate_kit_report;

    #[test]
    fn embedded_demo_kit_is_valid() {
        let kit = demo_kit().unwrap();
        let report = validate_kit_report(&kit);
        assert!(report.is_valid(), "{:#?}", report.issues);
        assert_eq!(kit.pads.len(), 16);
        assert!(kit.pads.iter().all(|pad| !pad.is_silent()));
        assert_eq!(kit.metadata.name, "Drumpad Demo");
    }

    #[test]
    fn every_referenced_sample_is_embedded() {
        let kit = demo_kit().unwrap();
        for sample in kit.samples() {
            let bytes = demo_sample_bytes(sample.path.as_str())
                .unwrap_or_else(|| panic!("missing embedded asset {}", sample.path));
            let (rate, channels, pcm) = decode_wav(bytes);
            assert_eq!(
                sample.hash,
                Some(crate::kit::ContentHash::of_pcm(rate, channels, &pcm)),
                "manifest content hash for {} does not match its decoded PCM",
                sample.path
            );
        }
    }

    fn decode_wav(bytes: &[u8]) -> (u32, u16, Vec<f32>) {
        let reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        let spec = reader.spec();
        let samples = reader
            .into_samples::<i16>()
            .map(|sample| sample.unwrap() as f32 / 32768.0)
            .collect();
        (spec.sample_rate, spec.channels, samples)
    }

    #[test]
    fn install_writes_manifest_and_samples() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("demo");
        let kit = install_demo_kit(&dest).unwrap();
        assert!(dest.join("kit.json").is_file());
        assert!(dest.join("LICENSES.md").is_file());
        assert!(dest.join("samples/kick.wav").is_file());
        assert_eq!(demo_kit().unwrap().pads, kit.pads);
        assert!(matches!(
            install_demo_kit(&dest),
            Err(DemoError::AlreadyInstalled(_))
        ));
    }

    #[test]
    fn missing_embedded_names_return_none() {
        assert!(demo_sample_bytes("samples/kick.wav").is_some());
        assert!(demo_sample_bytes("samples/nope.wav").is_none());
        assert!(demo_sample_bytes("").is_none());
    }

    #[test]
    fn install_reports_an_unusable_destination() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("already-a-file");
        std::fs::write(&file, b"not a folder").unwrap();
        let error = install_demo_kit(&file).unwrap_err();
        assert!(matches!(error, DemoError::Io { .. }));
    }
}
