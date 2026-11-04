//! Global hotkeys via a low-level keyboard hook (`SetWindowsHookExW`
//! with `WH_KEYBOARD_LL`).
//!
//! The hook runs on its own thread with a message loop. The hook procedure
//! only matches the chord against the current bindings and forwards the
//! action over a channel, so it never blocks input. Keys are passed through
//! to the game (not swallowed). Auto-repeat is suppressed so holding a chord
//! fires once.
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
        is_modifier_vk, Modifiers, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicU16, Ordering};
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

    fn down(vk: i32) -> bool {
        unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let vk = kb.vkCode as u16;
            let msg = wparam.0 as u32;
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
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
