//! Phase 4.2: WinEvent hooks for tracking popup menus (`#32768`), dropdowns (`ComboLBox`),
//! tooltips (`tooltips_class32`), and modal dialogs relative to their parent window quad.

use crate::scene::{Vec2, WindowQuad};
use parking_lot::Mutex;
use std::collections::HashMap;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetWindow, GetWindowRect, EVENT_OBJECT_CREATE,
    EVENT_OBJECT_DESTROY, EVENT_SYSTEM_MENUPOPUPEND, EVENT_SYSTEM_MENUPOPUPSTART, GA_ROOTOWNER,
    GW_OWNER, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

/// Metadata for a tracked transient popup / context menu window.
#[derive(Debug, Clone)]
pub struct TrackedPopup {
    pub popup_hwnd: isize,
    pub owner_hwnd: isize,
    pub offset_from_owner: Vec2,
    pub width: f32,
    pub height: f32,
}

static TRACKED_POPUPS: Mutex< Option<HashMap<isize, TrackedPopup>> > = Mutex::new(None);

/// Returns `true` if `hwnd` is a Win32 menu (`#32768`), combo box list, or tooltip popup.
pub fn is_popup_or_menu_class(hwnd: HWND) -> bool {
    let mut buf = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    if len == 0 {
        return false;
    }
    let class = String::from_utf16_lossy(&buf[..len]);
    matches!(
        class.as_str(),
        "#32768" | "ComboLBox" | "tooltips_class32" | "DropDown"
    )
}

unsafe extern "system" fn popup_win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _id_event_thread: u32,
    _dwms_event_time: u32,
) {
    if hwnd.0.is_null() || id_object != OBJID_WINDOW.0 {
        return;
    }

    match event {
        EVENT_OBJECT_CREATE | EVENT_SYSTEM_MENUPOPUPSTART => {
            if is_popup_or_menu_class(hwnd) {
                let owner = HierarchyTracker::get_root_owner(hwnd);
                let mut popup_rc = RECT::default();
                let mut owner_rc = RECT::default();
                unsafe {
                    let _ = GetWindowRect(hwnd, &mut popup_rc);
                    if !owner.0.is_null() {
                        let _ = GetWindowRect(owner, &mut owner_rc);
                    }
                }
                let offset = Vec2::new(
                    (popup_rc.left - owner_rc.left) as f32,
                    (popup_rc.top - owner_rc.top) as f32,
                );
                let width = (popup_rc.right - popup_rc.left).max(1) as f32;
                let height = (popup_rc.bottom - popup_rc.top).max(1) as f32;

                let mut guard = TRACKED_POPUPS.lock();
                let map = guard.get_or_insert_with(HashMap::new);
                map.insert(
                    hwnd.0 as isize,
                    TrackedPopup {
                        popup_hwnd: hwnd.0 as isize,
                        owner_hwnd: owner.0 as isize,
                        offset_from_owner: offset,
                        width,
                        height,
                    },
                );
            }
        }
        EVENT_OBJECT_DESTROY | EVENT_SYSTEM_MENUPOPUPEND => {
            let mut guard = TRACKED_POPUPS.lock();
            if let Some(map) = guard.as_mut() {
                map.remove(&(hwnd.0 as isize));
            }
        }
        _ => {}
    }
}

/// Tracks popup hierarchy so menus render above their owning window at the exact relative offset.
pub struct HierarchyTracker {
    hooks: Vec<HWINEVENTHOOK>,
}

impl HierarchyTracker {
    /// Starts listening to `EVENT_OBJECT_CREATE`, `EVENT_SYSTEM_MENUPOPUPSTART`,
    /// `EVENT_SYSTEM_MENUPOPUPEND`, and `EVENT_OBJECT_DESTROY`.
    pub fn start() -> Self {
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let mut hooks = Vec::new();
        unsafe {
            let h1 = SetWinEventHook(
                EVENT_SYSTEM_MENUPOPUPSTART,
                EVENT_SYSTEM_MENUPOPUPEND,
                None,
                Some(popup_win_event_proc),
                0,
                0,
                flags,
            );
            if !h1.is_invalid() {
                hooks.push(h1);
            }
            let h2 = SetWinEventHook(
                EVENT_OBJECT_CREATE,
                EVENT_OBJECT_DESTROY,
                None,
                Some(popup_win_event_proc),
                0,
                0,
                flags,
            );
            if !h2.is_invalid() {
                hooks.push(h2);
            }
        }
        Self { hooks }
    }

    /// Resolves the root owner `HWND` for a popup or modal child via `GA_ROOTOWNER` and `GW_OWNER`.
    pub fn get_root_owner(hwnd: HWND) -> HWND {
        unsafe {
            let owner = GetWindow(hwnd, GW_OWNER).unwrap_or_default();
            if !owner.0.is_null() {
                let root = GetAncestor(owner, GA_ROOTOWNER);
                if !root.0.is_null() {
                    return root;
                }
                return owner;
            }
            let root = GetAncestor(hwnd, GA_ROOTOWNER);
            if !root.0.is_null() && root != hwnd {
                root
            } else {
                HWND::default()
            }
        }
    }

    /// Computes a `WindowQuad` for a tracked popup positioned relative to its parent `owner_quad`
    /// in Vulkan coordinate space, elevated in Z-order directly above the parent.
    pub fn compute_relative_popup_quad(
        popup: &TrackedPopup,
        owner_quad: &WindowQuad,
    ) -> WindowQuad {
        let pos = Vec2::new(
            owner_quad.physics.position.x + popup.offset_from_owner.x,
            owner_quad.physics.position.y + popup.offset_from_owner.y,
        );
        let mut quad = WindowQuad::new(
            HWND(popup.popup_hwnd as *mut core::ffi::c_void),
            pos,
            popup.width,
            popup.height,
        );
        quad.z_order = owner_quad.z_order + 100;
        quad.workspace_id = owner_quad.workspace_id;
        quad
    }

    /// Returns a snapshot of all currently active popups.
    pub fn active_popups() -> Vec<TrackedPopup> {
        let guard = TRACKED_POPUPS.lock();
        guard
            .as_ref()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }
}

impl Drop for HierarchyTracker {
    fn drop(&mut self) {
        unsafe {
            for h in self.hooks.drain(..) {
                let _ = UnhookWinEvent(h);
            }
        }
    }
}
