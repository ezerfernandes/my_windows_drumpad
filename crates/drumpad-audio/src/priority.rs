//! Thread priority for the audio callback.
//!
//! WASAPI does not raise the priority of a render thread for you when you own
//! the event loop. Without MMCSS, a normal-priority thread can be preempted for
//! tens of milliseconds by compilers, indexing, or other agents on the machine,
//! which is an audible dropout. Register the thread with the “Pro Audio” task
//! and ask for critical priority.

/// Guard that keeps the calling thread in MMCSS “Pro Audio” for as long as it
/// is alive. Dropping it reverts the thread characteristics.
#[cfg(windows)]
pub struct MmcssGuard(Option<windows::Win32::Foundation::HANDLE>);

/// Boost the calling thread into the MMCSS “Pro Audio” class.
///
/// Returns a guard; if MMCSS is unavailable the guard is inert and the stream
/// still runs at normal priority.
#[cfg(windows)]
pub fn boost_audio_thread() -> MmcssGuard {
    use windows::Win32::System::Threading::{
        AVRT_PRIORITY_CRITICAL, AvSetMmThreadCharacteristicsA, AvSetMmThreadPriority,
    };
    let mut task_index = 0u32;
    let task_name = windows::core::PCSTR(c"Pro Audio".as_ptr().cast());
    let handle = unsafe { AvSetMmThreadCharacteristicsA(task_name, &mut task_index) };
    match handle {
        Ok(handle) => {
            unsafe {
                let _ = AvSetMmThreadPriority(handle, AVRT_PRIORITY_CRITICAL);
            }
            MmcssGuard(Some(handle))
        }
        Err(_) => MmcssGuard(None),
    }
}

#[cfg(windows)]
impl MmcssGuard {
    /// True when the thread entered the MMCSS class.
    pub fn is_active(&self) -> bool {
        self.0.is_some()
    }
}

#[cfg(windows)]
impl Drop for MmcssGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            unsafe {
                let _ = windows::Win32::System::Threading::AvRevertMmThreadCharacteristics(handle);
            }
        }
    }
}

#[cfg(not(windows))]
/// Inert guard on non-Windows platforms.
#[derive(Debug)]
pub struct MmcssGuard;

#[cfg(not(windows))]
/// No-op on non-Windows platforms.
pub fn boost_audio_thread() -> MmcssGuard {
    MmcssGuard
}

#[cfg(not(windows))]
impl MmcssGuard {
    /// Always false outside Windows.
    pub fn is_active(&self) -> bool {
        false
    }
}
