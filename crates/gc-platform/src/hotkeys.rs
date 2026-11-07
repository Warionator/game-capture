//! Global hotkeys via a low-level keyboard hook (`SetWindowsHookExW`
//! with `WH_KEYBOARD_LL`).
//!
//! The hook runs on its own thread with a message loop. The hook procedure
//! only matches the chord against the current bindings and forwards the
//! action over a channel, so it never blocks input. Keys are passed through
//! to the game (not swallowed). Auto-repeat is suppressed so holding a chord
//! fires once.
//!
//! The hook also notices the keys that take the player out of a fullscreen
//! game (Alt+Tab, Alt+Esc, the Windows key, Ctrl+Esc) so the focus screen
//! can go up before Windows switches windows (see [`switching_away`]).
//!
//! Note: Windows does not deliver input from an elevated (administrator)
//! foreground window to hooks in a non-elevated process (UIPI). Games run as
//! administrator therefore need game-capture to be elevated too.

use crossbeam_channel::Sender;
use gc_core::hotkey::{Hotkey, HotkeyAction};
use parking_lot::RwLock;
use std::sync::Arc;

pub type Bindings = Arc<RwLock<Vec<(Hotkey, HotkeyAction)>>>;

/// Handle to the running hook; dropping it uninstalls the hook.
pub struct HotkeyHook {
    #[cfg(windows)]
    thread_id: u32,
    #[cfg(windows)]
    thread: Option<std::thread::JoinHandle<()>>,
}

/// After the switch keys are released, how long the player counts as
/// switching away (Windows switches windows on release).
pub const SWITCH_TAIL: std::time::Duration = std::time::Duration::from_millis(250);

/// The player pressed a key combination that leaves the game (Alt+Tab,
/// Alt+Esc, the Windows key, Ctrl+Esc) and is still holding it, or let go
/// less than [`SWITCH_TAIL`] ago. Windows only switches windows when the
/// keys are released, so this comes ~100 ms before the foreground window
/// changes, and covers the Alt+Tab window list drawn over the game.
#[cfg(windows)]
pub fn switching_away() -> bool {
    imp::switching_away()
}
#[cfg(not(windows))]
pub fn switching_away() -> bool {
    false
}

#[cfg(not(windows))]
impl HotkeyHook {
    pub fn install(_bindings: Bindings, _tx: Sender<HotkeyAction>) -> anyhow::Result<Self> {
        log::warn!("global hotkeys are only available on Windows");
        Ok(Self {})
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use gc_core::hotkey::{
        is_modifier_vk, Modifiers, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LWIN, VK_MENU,
        VK_RCONTROL, VK_RMENU, VK_RWIN, VK_SHIFT,
    };
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
    use std::sync::OnceLock;
    use std::time::Instant;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
        WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    struct State {
        bindings: Bindings,
        tx: Sender<HotkeyAction>,
    }

    static STATE: Mutex<Option<State>> = Mutex::new(None);
    static HELD: AtomicU16 = AtomicU16::new(0);
    /// A switch-away combination is held down.
    static SWITCH_HELD: AtomicBool = AtomicBool::new(false);
    /// When it was let go: ms since `CLOCK` + 1 (0 = never).
    static SWITCH_RELEASED: AtomicU64 = AtomicU64::new(0);
    static CLOCK: OnceLock<Instant> = OnceLock::new();

    const VK_TAB: u16 = 0x09;
    const VK_ESCAPE: u16 = 0x1B;

    fn now_ms() -> u64 {
        CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    pub(super) fn switching_away() -> bool {
        if SWITCH_HELD.load(Ordering::Relaxed) {
            return true;
        }
        let released = SWITCH_RELEASED.load(Ordering::Relaxed);
        released != 0 && now_ms() + 1 < released + SWITCH_TAIL.as_millis() as u64
    }

    /// Tracks the switch-away keys; `vk` went down (`down`) or up.
    fn track_switch(vk: u16, down_now: bool) {
        let is_win = vk == VK_LWIN || vk == VK_RWIN;
        let is_alt = vk == VK_MENU || vk == VK_LMENU || vk == VK_RMENU;
        let is_ctrl = vk == VK_CONTROL || vk == VK_LCONTROL || vk == VK_RCONTROL;
        if down_now {
            let alt = down(VK_MENU as i32);
            let ctrl = down(VK_CONTROL as i32);
            if is_win || (alt && (vk == VK_TAB || vk == VK_ESCAPE)) || (ctrl && vk == VK_ESCAPE) {
                SWITCH_HELD.store(true, Ordering::Relaxed);
            }
        } else if (is_win || is_alt || is_ctrl) && SWITCH_HELD.swap(false, Ordering::Relaxed) {
            SWITCH_RELEASED.store(now_ms() + 1, Ordering::Relaxed);
        }
    }

    fn down(vk: i32) -> bool {
        unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let vk = kb.vkCode as u16;
            let msg = wparam.0 as u32;
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                track_switch(vk, true);
                if !is_modifier_vk(vk) && HELD.swap(vk, Ordering::Relaxed) != vk {
                    let mods = Modifiers {
                        ctrl: down(VK_CONTROL as i32),
                        alt: down(VK_MENU as i32),
                        shift: down(VK_SHIFT as i32),
                        win: down(VK_LWIN as i32) || down(VK_RWIN as i32),
                    };
                    if let Some(state) = STATE
                        .try_lock()
                        .as_ref()
                        .and_then(|s| s.as_ref().map(|s| (s.bindings.clone(), s.tx.clone())))
                    {
                        let action = state
                            .0
                            .read()
                            .iter()
                            .find(|(h, _)| h.matches(vk, mods))
                            .map(|(_, a)| *a);
                        if let Some(a) = action {
                            let _ = state.1.try_send(a);
                        }
                    }
                }
            } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
                track_switch(vk, false);
                let _ = HELD.compare_exchange(vk, 0, Ordering::Relaxed, Ordering::Relaxed);
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }

    impl HotkeyHook {
        pub fn install(bindings: Bindings, tx: Sender<HotkeyAction>) -> anyhow::Result<Self> {
            *STATE.lock() = Some(State { bindings, tx });
            let (ready_tx, ready_rx) = crossbeam_channel::bounded::<anyhow::Result<u32>>(1);
            let thread = std::thread::Builder::new()
                .name("gc-hotkeys".into())
                .spawn(move || unsafe {
                    let module = GetModuleHandleW(None).ok();
                    let hook = match SetWindowsHookExW(
                        WH_KEYBOARD_LL,
                        Some(hook_proc),
                        module.map(Into::into),
                        0,
                    ) {
                        Ok(h) => h,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.into()));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(GetCurrentThreadId()));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    let _ = UnhookWindowsHookEx(hook);
                })?;
            let thread_id = ready_rx.recv()??;
            Ok(Self {
                thread_id,
                thread: Some(thread),
            })
        }
    }

    impl Drop for HotkeyHook {
        fn drop(&mut self) {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            *STATE.lock() = None;
        }
    }
}
