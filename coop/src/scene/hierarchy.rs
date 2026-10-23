//! WinEvent hooks for tracking popup menus (#32768), dropdowns, and dialogs.

use windows::Win32::Foundation::HWND;

/// Tracks popup hierarchy so menus render above their owning window.
pub struct HierarchyTracker;

impl HierarchyTracker {
    /// Starts listening to `EVENT_OBJECT_CREATE` and `EVENT_SYSTEM_MENUPOPUPSTART`.
    pub fn start() {
        // Skeleton: To be implemented by Gemini 4 Argon
    }

    /// Resolves the root owner HWND for a popup or modal child.
    pub fn get_root_owner(hwnd: HWND) -> HWND {
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetAncestor(hwnd, windows::Win32::UI::WindowsAndMessaging::GA_ROOTOWNER) }
    }
}
