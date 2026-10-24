//! Notification-area icon: its own thread runs a hidden window that receives clicks and the context menu.

use std::sync::{Arc, atomic::AtomicIsize, mpsc::Sender};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    Open,
    Quit,
}

/// Removes the icon when dropped.
pub struct TrayIcon {
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: isize,
}

/// Shows the icon; each click or menu choice goes to `events`, then `wake` runs (repaint the UI).
/// `main` holds the app window handle (0 until known): "Open" restores it from the tray thread itself,
/// since a hidden egui window no longer runs its frame loop.
pub fn spawn(events: Sender<TrayEvent>, wake: Box<dyn Fn() + Send>, main: Arc<AtomicIsize>) -> anyhow::Result<TrayIcon> {
    platform::spawn(events, wake, main).map(|hwnd| TrayIcon { hwnd })
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        platform::close(self.hwnd);
    }
}

#[cfg(windows)]
// The SINK thread-local is already `const`; the lint misfires inside the macro expansion.
#[allow(clippy::missing_const_for_thread_local)]
mod platform {
    use std::{
        cell::RefCell,
        sync::{
            Arc,
            atomic::{AtomicIsize, Ordering},
            mpsc::{self, Sender},
        },
        thread,
    };

    use anyhow::{Result, anyhow};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW},
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW,
                GetCursorPos, GetMessageW, HWND_MESSAGE, LoadIconW, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage,
                RegisterClassW, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON,
                TrackPopupMenu, TranslateMessage, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WM_RBUTTONUP,
                WNDCLASSW,
            },
        },
    };

    use super::TrayEvent;

    const CALLBACK: u32 = WM_APP + 1;
    const OPEN: usize = 1;
    const QUIT: usize = 2;

    struct Sink {
        events: Sender<TrayEvent>,
        wake: Box<dyn Fn() + Send>,
        main: Arc<AtomicIsize>,
    }

    thread_local! {
        static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn emit(event: TrayEvent) {
        SINK.with_borrow(|s| {
            let Some(s) = s else { return };
            let main = s.main.load(Ordering::Acquire);
            if event == TrayEvent::Open && main != 0 {
                // SAFETY: plain calls on a top-level window handle; a stale handle only makes them fail.
                unsafe {
                    ShowWindow(main as HWND, SW_SHOW);
                    ShowWindow(main as HWND, SW_RESTORE);
                    SetForegroundWindow(main as HWND);
                }
            }
            let _ = s.events.send(event);
            (s.wake)();
        });
    }

    pub fn spawn(events: Sender<TrayEvent>, wake: Box<dyn Fn() + Send>, main: Arc<AtomicIsize>) -> Result<isize> {
        let (ready, created) = mpsc::channel();
        thread::Builder::new().name("tray".into()).spawn(move || {
            SINK.with_borrow_mut(|s| *s = Some(Sink { events, wake, main }));
            let hwnd = create();
            let _ = ready.send(hwnd);
            if hwnd.is_some() {
                pump();
            }
        })?;
        created.recv()?.ok_or_else(|| anyhow!("icône de notification indisponible"))
    }

    pub fn close(hwnd: isize) {
        // SAFETY: posting to a window of another thread is allowed; a stale handle makes the call fail harmlessly.
        unsafe { PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0) };
    }

    fn data(hwnd: HWND) -> NOTIFYICONDATAW {
        // SAFETY: NOTIFYICONDATAW is plain data; all-zero is its documented empty state.
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = u32::try_from(size_of::<NOTIFYICONDATAW>()).unwrap_or(0);
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid
    }

    fn create() -> Option<isize> {
        let class = wide("OmegaTray");
        // SAFETY: Win32 calls with NUL-terminated buffers alive for the call; handles checked before use.
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let wc =
                WNDCLASSW { lpfnWndProc: Some(proc), hInstance: instance, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
            RegisterClassW(&raw const wc);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return None;
            }
            let mut nid = data(hwnd);
            nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = CALLBACK;
            // Resource 1 = the application icon embedded by build.rs.
            nid.hIcon = LoadIconW(instance, 1 as _);
            let tip = wide("Omega");
            nid.szTip[..tip.len()].copy_from_slice(&tip);
            if Shell_NotifyIconW(NIM_ADD, &raw const nid) == 0 {
                DestroyWindow(hwnd);
                return None;
            }
            Some(hwnd as isize)
        }
    }

    fn pump() {
        // SAFETY: standard message loop on the thread that owns the window.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&raw mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&raw const msg);
                DispatchMessageW(&raw const msg);
            }
        }
    }

    fn menu(hwnd: HWND) {
        // SAFETY: menu created, shown and destroyed on this thread; buffers alive for each call.
        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            AppendMenuW(menu, MF_STRING, OPEN, wide("Ouvrir Omega").as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(menu, MF_STRING, QUIT, wide("Quitter").as_ptr());
            let mut at = POINT { x: 0, y: 0 };
            GetCursorPos(&raw mut at);
            // Without it the menu would not close when clicking elsewhere.
            SetForegroundWindow(hwnd);
            let choice = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, 0, hwnd, std::ptr::null());
            DestroyMenu(menu);
            match usize::try_from(choice).unwrap_or(0) {
                OPEN => emit(TrayEvent::Open),
                QUIT => emit(TrayEvent::Quit),
                _ => {}
            }
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            CALLBACK => {
                match (lparam & 0xFFFF) as u32 {
                    WM_LBUTTONUP => emit(TrayEvent::Open),
                    WM_RBUTTONUP | WM_CONTEXTMENU => menu(hwnd),
                    _ => {}
                }
                0
            }
            WM_CLOSE => {
                // SAFETY: icon and window owned by this thread.
                unsafe {
                    let nid = data(hwnd);
                    Shell_NotifyIconW(NIM_DELETE, &raw const nid);
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_DESTROY => {
                // SAFETY: ends this thread's message loop.
                unsafe { PostQuitMessage(0) };
                0
            }
            // SAFETY: default handling for every other message.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::sync::{Arc, atomic::AtomicIsize, mpsc::Sender};

    use super::TrayEvent;

    pub fn spawn(_: Sender<TrayEvent>, _: Box<dyn Fn() + Send>, _: Arc<AtomicIsize>) -> anyhow::Result<isize> {
        anyhow::bail!("zone de notification non prise en charge")
    }

    pub fn close(_: isize) {}
}
