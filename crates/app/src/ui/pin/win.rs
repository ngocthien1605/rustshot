//! The pinned-image window: borderless (a thin teal frame), always on top, no taskbar button.
//! Drag it anywhere to move it; the mouse wheel zooms around the pointer; right-click for Copy,
//! Actual size and Close; double-click or `Esc` closes it. Several can be open at once, each
//! owning its own copy of the image (deleting the file does not affect an open pin).

use std::cell::RefCell;
use std::path::Path;
use std::sync::Once;

use anyhow::{anyhow, Context, Result};
use capture_core::Rect;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FrameRect, InvalidateRect,
    SetStretchBltMode, StretchBlt, HALFTONE, HGDIOBJ, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_CONTROL, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    GetClientRect, GetCursorPos, GetWindowLongPtrW, GetWindowRect, LoadCursorW, RegisterClassW,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, TrackPopupMenu,
    GWLP_USERDATA, HMENU, HTCAPTION, IDC_SIZEALL, MF_SEPARATOR, MF_STRING, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_SHOW, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_DPICHANGED, WM_ERASEBKGND,
    WM_KEYDOWN, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK,
    WM_NCRBUTTONUP, WM_PAINT, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::platform::gdi::{rgba_from_surface, surface_from_rgba, Surface};

use super::model;

const CLASS_NAME: PCWSTR = w!("RustShotPin");
static REGISTER: Once = Once::new();

/// The frame that tells a pinned image apart from the window under it (RustShot's teal).
const FRAME: COLORREF = COLORREF(0x0088_9600); // 0x00BBGGRR

// Context-menu commands.
const CMD_COPY: u32 = 1;
const CMD_ACTUAL: u32 = 2;
const CMD_CLOSE: u32 = 3;

/// Per-window state, owned by the window (freed in `WM_NCDESTROY`).
struct PinCtx {
    /// The image as a DIB-backed memory DC; also the source for Copy.
    src: Surface,
    width: i32,
    height: i32,
    /// Index into [`model::ZOOMS`].
    zoom: usize,
}

thread_local! {
    /// The state of a window being created, until its `WM_NCCREATE` adopts it (as in the
    /// annotation editor: whatever is left here after creation fails is dropped normally).
    static PENDING: RefCell<Option<Box<PinCtx>>> = const { RefCell::new(None) };
}

/// Pin the image file at `path`. Returns immediately; the window lives on the UI thread's
/// message loop until the user closes it. Failures are reported to the user.
pub fn open(path: &Path) {
    if let Err(e) = try_open(path) {
        crate::notify::error("The screenshot could not be pinned.", e);
    }
}

fn try_open(path: &Path) -> Result<()> {
    ensure_class()?;
    let image = imaging::load_rgba(path).context("loading the screenshot")?;
    let (iw, ih) = (image.width() as i32, image.height() as i32);
    let src = surface_from_rgba(&image)?;
    drop(image); // the surface holds the pixels from here on

    let (work, _) = crate::platform::dpi::cursor_monitor();
    let (ww, wh) = (work.right - work.left, work.bottom - work.top);
    let zoom = model::initial_zoom(iw, ih, ww, wh);
    let (w, h) = model::size_at(iw, ih, zoom);
    let ctx = Box::new(PinCtx {
        src,
        width: iw,
        height: ih,
        zoom,
    });

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = HSTRING::from(format!("RustShot — pinned — {name}"));
    // SAFETY: window creation; the window adopts `ctx` in WM_NCCREATE (via PENDING) and frees
    // it in WM_NCDESTROY.
    let created = unsafe {
        let hinstance = GetModuleHandleW(None).context("GetModuleHandleW")?;
        PENDING.with(|p| *p.borrow_mut() = Some(ctx));
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS_NAME,
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            work.left + ((ww - w) / 2).max(0),
            work.top + ((wh - h) / 2).max(0),
            w,
            h,
            None,
            Some(HMENU(std::ptr::null_mut())),
            Some(hinstance.into()),
            None,
        )
    };
    drop(PENDING.with(|p| p.borrow_mut().take()));
    let hwnd = created.context("creating the pinned window")?;
    // SAFETY: show and focus our own window, so Esc and Ctrl+C reach it.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some(hwnd));
    }
    tracing::info!("pinned {}", path.display());
    Ok(())
}

fn ensure_class() -> Result<()> {
    let mut err: Option<windows::core::Error> = None;
    REGISTER.call_once(|| {
        // SAFETY: one-time class registration.
        unsafe {
            let hinstance = match GetModuleHandleW(None) {
                Ok(h) => h,
                Err(e) => {
                    err = Some(e);
                    return;
                }
            };
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance.into(),
                lpszClassName: CLASS_NAME,
                // The whole window is a drag handle: a move cursor says so.
                hCursor: LoadCursorW(None, IDC_SIZEALL).unwrap_or_default(),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                err = Some(windows::core::Error::from_thread());
            }
        }
    });
    match err {
        Some(e) => Err(e).context("registering the pinned window class"),
        None => Ok(()),
    }
}

fn ctx_of<'a>(hwnd: HWND) -> Option<&'a mut PinCtx> {
    // SAFETY: userdata is null or the Box<PinCtx> installed in WM_NCCREATE and freed in
    // WM_NCDESTROY (after which no more messages arrive).
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut PinCtx;
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A panic must not unwind across this FFI boundary (that aborts the process).
    crate::panic_guard::guard(
        "pinned window wndproc",
        || unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        || unsafe { wndproc_inner(hwnd, msg, wparam, lparam) },
    )
}

unsafe fn wndproc_inner(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        if let Some(ctx) = PENDING.with(|p| p.borrow_mut().take()) {
            // SAFETY: store the owned pointer in our own window's userdata.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(ctx) as isize) };
        }
        // SAFETY: default processing of WM_NCCREATE.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    if msg == WM_NCDESTROY {
        // SAFETY: last message: take back ownership and free the state exactly once.
        unsafe {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PinCtx;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
    }
    if ctx_of(hwnd).is_none() {
        // SAFETY: default handling before/after our state exists.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    match msg {
        // Anywhere in the window drags it, like a title bar.
        WM_NCHITTEST => LRESULT(HTCAPTION as isize),
        WM_NCLBUTTONDBLCLK => {
            close(hwnd);
            LRESULT(0)
        }
        WM_NCRBUTTONUP => {
            context_menu(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wparam.0 as u16;
            // SAFETY: reads a modifier's state for the message being processed.
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            if vk == VK_ESCAPE.0 {
                close(hwnd);
            } else if ctrl && vk == b'C' as u16 {
                copy(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16;
            // The pointer, in screen coordinates for this message.
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            if delta != 0 {
                let zoom = ctx_of(hwnd).map(|c| model::step(c.zoom, delta > 0));
                if let Some(zoom) = zoom {
                    set_zoom(hwnd, zoom, Some((x, y)));
                }
            }
            LRESULT(0)
        }
        // Keep the size when moved to a monitor with another scale: the image's pixels are the
        // screen's pixels, so 100 % should stay 100 %.
        WM_DPICHANGED => LRESULT(0),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        // SAFETY: default handling.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Resize to zoom step `zoom`, keeping `anchor` (screen px; the window's centre if `None`) over
/// the same spot of the image.
fn set_zoom(hwnd: HWND, zoom: usize, anchor: Option<(i32, i32)>) {
    let Some(ctx) = ctx_of(hwnd) else {
        return;
    };
    if zoom == ctx.zoom {
        return;
    }
    ctx.zoom = zoom;
    let (w, h) = model::size_at(ctx.width, ctx.height, zoom);
    let mut r = RECT::default();
    // SAFETY: reads our own window rectangle, then moves/resizes our own window. The state is
    // not used after SetWindowPos, which can send messages back to this window.
    unsafe {
        if GetWindowRect(hwnd, &mut r).is_err() {
            return;
        }
        let old = Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top);
        let (ax, ay) = anchor.unwrap_or((old.x + old.width / 2, old.y + old.height / 2));
        let new = model::zoom_around(old, w, h, ax, ay);
        let _ = SetWindowPos(
            hwnd,
            None,
            new.x,
            new.y,
            new.width,
            new.height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

fn paint(hwnd: HWND) {
    let Some(ctx) = ctx_of(hwnd) else {
        return;
    };
    // SAFETY: standard paint: one scaled blit of the image, then a 1-px frame.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        SetStretchBltMode(hdc, HALFTONE);
        let _ = StretchBlt(
            hdc,
            0,
            0,
            rc.right,
            rc.bottom,
            Some(ctx.src.dc),
            0,
            0,
            ctx.width,
            ctx.height,
            SRCCOPY,
        );
        let brush = CreateSolidBrush(FRAME);
        FrameRect(hdc, &rc, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        let _ = EndPaint(hwnd, &ps);
    }
}

/// Copy, Actual size, Close. The menu pumps messages while open, so no reference to the state
/// is held across it; the chosen command is carried out afterwards.
fn context_menu(hwnd: HWND) {
    // SAFETY: a popup menu owned and destroyed here; TrackPopupMenu with TPM_RETURNCMD returns
    // the chosen command instead of posting it.
    let cmd = unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let _ = AppendMenuW(menu, MF_STRING, CMD_COPY as usize, w!("Copy\tCtrl+C"));
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            CMD_ACTUAL as usize,
            w!("Actual size (100 %)"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, CMD_CLOSE as usize, w!("Close\tEsc"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Without this the menu does not close when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        chosen.0 as u32
    };
    match cmd {
        CMD_COPY => copy(hwnd),
        CMD_ACTUAL => set_zoom(hwnd, model::ACTUAL_SIZE, None),
        CMD_CLOSE => close(hwnd),
        _ => {}
    }
}

/// Put the pinned image (at full size, whatever the zoom) on the clipboard.
fn copy(hwnd: HWND) {
    let Some(ctx) = ctx_of(hwnd) else {
        return;
    };
    let result = rgba_from_surface(&ctx.src, ctx.width as u32, ctx.height as u32)
        .ok_or_else(|| anyhow!("the pinned image could not be read back"))
        .and_then(|img| crate::clipboard::copy_image(&img));
    if let Err(e) = result {
        crate::notify::warn("The pinned image could not be copied to the clipboard.", e);
    }
}

fn close(hwnd: HWND) {
    // SAFETY: destroy our own window; WM_NCDESTROY frees its state.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}
