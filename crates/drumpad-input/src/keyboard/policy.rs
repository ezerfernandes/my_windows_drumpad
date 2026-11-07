//! Global keyboard capture policy.
//!
//! Drumpad receives keys only while its window is focused, which is the safe
//! default for a stage instrument. The optional Windows low-level hook captures
//! keys system-wide; enabling it requires an explicit consent token so no code
//! path can turn it on by accident (a boolean argument is too easy to pass by
//! mistake).

use serde::{Deserialize, Serialize};

/// Proof that the user explicitly opted into system-wide capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureConsent {
    /// The user confirmed system-wide capture in Settings.
    UserConfirmed,
}

/// Whether the optional global hook may run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalCapturePolicy {
    enabled: bool,
}

impl GlobalCapturePolicy {
    /// Policy with global capture disabled (the default).
    #[must_use]
    pub const fn disabled() -> Self {
        Self { enabled: false }
    }

    /// True when the user opted into global capture.
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        self.enabled
    }

    /// Enable global capture with explicit user consent.
    pub fn enable(&mut self, consent: CaptureConsent) {
        let CaptureConsent::UserConfirmed = consent;
        self.enabled = true;
    }

    /// Return to the default focused-window-only behavior.
    pub fn disable(&mut self) {
        self.enabled = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_capture_is_off_by_default() {
        assert!(!GlobalCapturePolicy::default().is_enabled());
        assert!(!GlobalCapturePolicy::disabled().is_enabled());
    }

    #[test]
    fn enabling_requires_an_explicit_consent_token() {
        let mut policy = GlobalCapturePolicy::default();
        policy.enable(CaptureConsent::UserConfirmed);
        assert!(policy.is_enabled());
        policy.disable();
        assert!(!policy.is_enabled());
    }

    #[test]
    fn policy_round_trips_through_json() {
        let mut policy = GlobalCapturePolicy::default();
        policy.enable(CaptureConsent::UserConfirmed);
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(
            serde_json::from_str::<GlobalCapturePolicy>(&json).unwrap(),
            policy
        );
    }
}
