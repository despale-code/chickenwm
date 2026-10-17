//! Single-instance guard for the daemon.
//!
//! Two daemons can start almost together: when rooster is the configured Winlogon shell and
//! kills explorer, Winlogon (`AutoRestartShell`) relaunches the shell — a second rooster. Both
//! would spawn every module and fight over the IPC pipe, so the loser's failsafe would tear the
//! whole shell down. A session-wide mutex, claimed before anything else happens, decides which
//! one runs.

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject, INFINITE};

/// Ownership of the daemon mutex; released when dropped or when the process exits.
pub struct InstanceGuard(HANDLE);

// The mutex handle is only ever released from the thread that owns the guard.
unsafe impl Send for InstanceGuard {}

impl InstanceGuard {
    /// Lets another daemon take over right away (before this process has exited).
    pub fn release(self) {}
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

/// Outcome of [`claim_daemon_instance`].
pub enum Instance {
    /// This process is the daemon.
    Primary(InstanceGuard),
    /// Another daemon is running. `wait_for_exit` blocks until it is gone.
    Duplicate(Option<HANDLE>),
}

impl Instance {
    /// For a duplicate: block until the running daemon exits (or return at once if it can't be
    /// observed).
    pub fn wait_for_exit(&self) {
        if let Instance::Duplicate(Some(h)) = self {
            unsafe {
                WaitForSingleObject(*h, INFINITE);
            }
        }
    }
}

/// Tries to become the daemon. `wait_ms` is how long to wait for a previous daemon to let go
/// (0 for a normal start, a few seconds when taking over from one that is exiting).
pub fn claim_daemon_instance(wait_ms: u32) -> Instance {
    claim_instance("Local\\ChickenWMRoosterDaemon", wait_ms)
}

/// Claims the session-wide mutex `name` (e.g. `Local\ChickenWMShot`) for a single-instance tool.
pub fn claim_instance(name: &str, wait_ms: u32) -> Instance {
    unsafe {
        // Interactive users may open it too, so a normal process can see an elevated daemon.
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let have_sd = ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;IU)"),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
        .is_ok();
        let attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let handle = CreateMutexW(
            have_sd.then_some(&attrs as *const SECURITY_ATTRIBUTES),
            false,
            &windows::core::HSTRING::from(name),
        );
        if have_sd {
            let _ = LocalFree(HLOCAL(sd.0));
        }
        let Ok(handle) = handle else {
            // Exists but we may not open it (made by an older build with a default ACL).
            return Instance::Duplicate(None);
        };
        let r = WaitForSingleObject(handle, wait_ms);
        if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
            Instance::Primary(InstanceGuard(handle))
        } else {
            Instance::Duplicate(Some(handle))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_claim_is_a_duplicate_until_the_first_releases() {
        // A name of its own, so a running ChickenWM doesn't interfere.
        const NAME: &str = r"Local\ChickenWMInstanceTest";
        let first = match claim_instance(NAME, 0) {
            Instance::Primary(g) => g,
            Instance::Duplicate(_) => panic!("first claim should win"),
        };
        // Mutexes are owned per thread, so claim from another one like a second process would.
        let second = std::thread::spawn(|| matches!(claim_instance(NAME, 0), Instance::Primary(_)));
        assert!(!second.join().unwrap(), "second claim must be a duplicate");
        first.release();
        let third = std::thread::spawn(|| matches!(claim_instance(NAME, 0), Instance::Primary(_)));
        assert!(third.join().unwrap(), "claim succeeds after release");
    }
}
