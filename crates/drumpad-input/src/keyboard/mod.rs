//! Keyboard and single-hand keypad capture.
//!
//! The default 4x4 pad grid is the physical block:
//!
//! ```text
//! 1 2 3 4        Q W E R        A S D F        Z X C V
//! ```
//!
//! with `F1` panic, `F2` metronome, `F3` previous kit, `F4` next kit, `F5` map
//! overlay. The fifth column (`5 T G B`) and `Space` stay unmapped so a palm
//! hit cannot fire a pad. The same grid carries over to the Finding 35-key
//! single-hand USB keypad (ASIN B09HF5WDR2), whose QWERTY block mirrors it.
//!
//! Everything here is layout independent: profiles store [`PhysicalKey`]
//! values, never characters. The app converts its platform key codes at the
//! edge.
//!
//! ```no_run
//! use drumpad_input::keyboard::{KeyboardInput, KeyboardProfile};
//!
//! let profile = KeyboardProfile::finding_35_key();
//! let mut keyboard = KeyboardInput::default_with_profile(&profile);
//! ```

pub mod editor;
pub mod input;
pub mod keys;
pub mod map;
pub mod policy;
pub mod profile;
pub mod velocity;

pub use editor::{CaptureOutcome, ConflictPolicy, MappingEditor, ProfileLibrary};
pub use input::{IgnoreReason, KeyboardAction, KeyboardInput, ReleaseAll};
pub use keys::PhysicalKey;
pub use map::KeyMap;
pub use policy::{CaptureConsent, GlobalCapturePolicy};
pub use profile::{
    FINDING_35_PROFILE_ID, FULL_KEYBOARD_PROFILE_ID, KeyAction, KeyBinding, KeyboardProfile,
    MappingConflict, PROFILE_SCHEMA_VERSION, ProfileDevice, ProfileError, TransportAction,
};
pub use velocity::{KeyboardVelocityConfig, VelocityGenerator, VelocityMode};
