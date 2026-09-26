// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
#![allow(unsafe_code)]
use crossbeam_queue::ArrayQueue;
use efude_core::InkPoint;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub struct PenPacket {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rotation: f32,
    pub time_ms: u64,
    pub received_at: std::time::Instant,
}

/// Estimates a short-lived preview point from actual input samples.
///
/// This fallback is for rendering guidance only. Do not enqueue its result or
/// append it to stroke samples; an OS prediction provider can replace it.
pub fn estimate_preview_point(
    points: &[InkPoint],
    horizon_ms: f32,
    max_offset: f32,
) -> Option<InkPoint> {
    if !horizon_ms.is_finite() || horizon_ms <= 0.0 || !max_offset.is_finite() || max_offset <= 0.0
    {
        return None;
    }
    let current = *points.last()?;
    let previous = *points.get(points.len().checked_sub(2)?)?;
    let elapsed_ms = current.time_ms.saturating_sub(previous.time_ms).max(1) as f32;
    let offset = ((current.position - previous.position) / elapsed_ms * horizon_ms)
        .clamp_length_max(max_offset);
    if !offset.is_finite() || offset.length() < 0.1 {
        return None;
    }
    let pressure_delta = current.pressure - previous.pressure;
    Some(InkPoint {
        position: current.position + offset,
        pressure: (current.pressure + pressure_delta / elapsed_ms * horizon_ms).clamp(0.0, 1.0),
        time_ms: current.time_ms.saturating_add(horizon_ms as u64),
        ..current
    })
}

pub struct PenInputQueue(Arc<ArrayQueue<PenPacket>>);
impl PenInputQueue {
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(ArrayQueue::new(capacity.max(8))))
    }
    pub fn push(&self, packet: PenPacket) {
        if self.0.push(packet).is_err() {
            let _ = self.0.pop();
            let _ = self.0.push(packet);
        }
    }
    pub fn pop(&self) -> Option<PenPacket> {
        self.0.pop()
    }
    pub fn clear(&self) {
        while self.0.pop().is_some() {}
    }
    pub fn drain_into(&self, output: &mut Vec<PenPacket>, limit: usize) -> bool {
        output.clear();
        while output.len() < limit.max(1)
            && let Some(packet) = self.0.pop()
        {
            output.push(packet);
        }
        !self.0.is_empty()
    }
    pub fn shared(&self) -> Arc<ArrayQueue<PenPacket>> {
        Arc::clone(&self.0)
    }
}
impl Default for PenInputQueue {
    fn default() -> Self {
        Self::new(16_384)
    }
}

#[cfg(target_os = "windows")]
mod windows_ink {
    use super::{Arc, ArrayQueue, PenPacket};
    use std::ffi::c_void;
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::ScreenToClient,
        UI::{
            HiDpi::GetDpiForWindow,
            Input::Pointer::{GetPointerPenInfo, GetPointerPenInfoHistory, POINTER_PEN_INFO},
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{
                PEN_MASK_PRESSURE, PEN_MASK_TILT_X, PEN_MASK_TILT_Y, WM_POINTERDOWN, WM_POINTERUP,
                WM_POINTERUPDATE,
            },
        },
    };

    struct HookState {
        queue: Arc<ArrayQueue<PenPacket>>,
    }
    const SUBCLASS_ID: usize = 0x4546554445;

    fn push_info(hwnd: HWND, state: &HookState, info: &POINTER_PEN_INFO) {
        let mut point = info.pointerInfo.ptPixelLocation;
        if unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
            let dpi = unsafe { GetDpiForWindow(hwnd) }.max(1) as f32 / 96.;
            let packet = PenPacket {
                x: point.x as f32 / dpi,
                y: point.y as f32 / dpi,
                pressure: if info.penMask & PEN_MASK_PRESSURE != 0 {
                    info.pressure as f32 / 1024.
                } else {
                    1.
                },
                tilt_x: if info.penMask & PEN_MASK_TILT_X != 0 {
                    info.tiltX as f32 / 90.
                } else {
                    0.
                },
                tilt_y: if info.penMask & PEN_MASK_TILT_Y != 0 {
                    info.tiltY as f32 / 90.
                } else {
                    0.
                },
                rotation: (info.rotation as f32).to_radians(),
                time_ms: info.pointerInfo.dwTime as u64,
                received_at: std::time::Instant::now(),
            };
            let _ = state.queue.push(packet);
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        data: usize,
    ) -> LRESULT {
        if matches!(msg, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP) {
            let pointer_id = (wparam.0 & 0xffff) as u32;
            let state = unsafe { &*(data as *const HookState) };
            let mut history_count = 0u32;
            if msg == WM_POINTERUPDATE
                && unsafe { GetPointerPenInfoHistory(pointer_id, &mut history_count, None) }.is_ok()
                && history_count > 0
            {
                // Keep a bounded recent history, then reverse the API's
                // newest-first ordering before feeding the stroke queue.
                let capacity = history_count.min(256) as usize;
                let mut history = vec![POINTER_PEN_INFO::default(); capacity];
                let mut available = capacity as u32;
                if unsafe {
                    GetPointerPenInfoHistory(pointer_id, &mut available, Some(history.as_mut_ptr()))
                }
                .is_ok()
                {
                    history.truncate(available.min(capacity as u32) as usize);
                    for info in history.iter().rev() {
                        push_info(hwnd, state, info);
                    }
                    return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
                }
            }
            let mut info = POINTER_PEN_INFO::default();
            if unsafe { GetPointerPenInfo(pointer_id, &mut info) }.is_ok() {
                push_info(hwnd, state, &info);
            }
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    pub struct WindowsInkHook {
        hwnd: HWND,
        state: Box<HookState>,
    }
    impl WindowsInkHook {
        pub fn install(
            hwnd: *mut c_void,
            queue: Arc<ArrayQueue<PenPacket>>,
        ) -> Result<Self, windows::core::Error> {
            let hwnd = HWND(hwnd);
            let state = Box::new(HookState { queue });
            let data = (&*state as *const HookState) as usize;
            if !unsafe { SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID, data) }.as_bool() {
                return Err(windows::core::Error::from_win32());
            }
            Ok(Self { hwnd, state })
        }
    }
    impl Drop for WindowsInkHook {
        fn drop(&mut self) {
            let _keep_state_alive = &self.state;
            unsafe {
                let _ = RemoveWindowSubclass(self.hwnd, Some(window_proc), SUBCLASS_ID);
            }
        }
    }
}

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
#[cfg(target_os = "windows")]
pub use windows_ink::WindowsInkHook;

/// Copies an RGBA image to the Windows clipboard using a DIBV5 bitmap.
/// Whether Ctrl and the key with virtual-key code `key` (an upper-case
/// letter or digit) are both held down right now. The window system turns
/// Ctrl+V into a text paste and drops the key when the clipboard holds an
/// image, so the paste shortcut is also watched here.
#[cfg(target_os = "windows")]
pub fn ctrl_key_down(key: u8) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL};
    // SAFETY: GetAsyncKeyState only reads the keyboard state.
    unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) < 0 && GetAsyncKeyState(key as i32) < 0 }
}

#[cfg(target_os = "windows")]
pub fn set_clipboard_image(width: u32, height: u32, rgba: &[u8]) -> windows::core::Result<()> {
    use windows::Win32::{
        Foundation::{GlobalFree, HANDLE, HWND},
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        },
    };

    const MAX_PIXELS: usize = 100_000_000;
    const HEADER_SIZE: usize = 124;
    const CF_DIBV5: u32 = 17;

    let pixels = (width as usize)
        .checked_mul(height as usize)
        .filter(|count| *count > 0 && *count <= MAX_PIXELS)
        .ok_or_else(windows::core::Error::from_win32)?;
    if rgba.len() != pixels.saturating_mul(4) {
        return Err(windows::core::Error::from_win32());
    }
    let data_size = pixels * 4;
    let mut dib = vec![0; HEADER_SIZE + data_size];
    dib[0..4].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
    dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    dib[8..12].copy_from_slice(&(-(height as i32)).to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32u16.to_le_bytes());
    dib[16..20].copy_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
    dib[20..24].copy_from_slice(&(data_size as u32).to_le_bytes());
    for (offset, mask) in [0x00ff_0000u32, 0x0000_ff00, 0x0000_00ff, 0xff00_0000]
        .into_iter()
        .enumerate()
    {
        let start = 40 + offset * 4;
        dib[start..start + 4].copy_from_slice(&mask.to_le_bytes());
    }
    dib[56..60].copy_from_slice(&0x7352_4742u32.to_le_bytes()); // LCS_sRGB
    for (source, target) in rgba
        .chunks_exact(4)
        .zip(dib[HEADER_SIZE..].chunks_exact_mut(4))
    {
        target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
    }

    unsafe { OpenClipboard(HWND(std::ptr::null_mut()))? };
    let result = (|| unsafe {
        EmptyClipboard()?;
        let memory = GlobalAlloc(GMEM_MOVEABLE, dib.len())?;
        let destination = GlobalLock(memory);
        if destination.is_null() {
            let _ = GlobalFree(memory);
            return Err(windows::core::Error::from_win32());
        }
        std::ptr::copy_nonoverlapping(dib.as_ptr(), destination.cast::<u8>(), dib.len());
        let _ = GlobalUnlock(memory);
        match SetClipboardData(CF_DIBV5, HANDLE(memory.0)) {
            Ok(_) => Ok(()),
            Err(error) => {
                let _ = GlobalFree(memory);
                Err(error)
            }
        }
    })();
    let _ = unsafe { CloseClipboard() };
    result
}

/// Reads a Windows DIB/DIBV5 image and converts it to RGBA8.
#[cfg(target_os = "windows")]
pub fn get_clipboard_image() -> windows::core::Result<Option<(u32, u32, Vec<u8>)>> {
    use windows::Win32::{
        Foundation::HWND,
        System::{
            DataExchange::{
                CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
            },
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        },
    };

    const MAX_PIXELS: usize = 100_000_000;
    unsafe { OpenClipboard(HWND(std::ptr::null_mut()))? };
    let result = (|| unsafe {
        let format = if IsClipboardFormatAvailable(17).is_ok() {
            17 // CF_DIBV5
        } else if IsClipboardFormatAvailable(8).is_ok() {
            8 // CF_DIB
        } else {
            return Ok(None);
        };
        let handle = GetClipboardData(format)?;
        let memory = windows::Win32::Foundation::HGLOBAL(handle.0);
        let size = GlobalSize(memory);
        if size < 40 || size > MAX_PIXELS.saturating_mul(8).saturating_add(4096) {
            return Ok(None);
        }
        let source = GlobalLock(memory);
        if source.is_null() {
            return Ok(None);
        }
        let bytes = std::slice::from_raw_parts(source.cast::<u8>(), size);
        let header_size = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let width = i32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let signed_height = i32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let planes = u16::from_le_bytes(bytes[12..14].try_into().unwrap());
        let bits_per_pixel = u16::from_le_bytes(bytes[14..16].try_into().unwrap());
        let compression = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let parsed = (|| {
            if header_size < 40
                || header_size > size
                || planes != 1
                || width <= 0
                || signed_height == 0
                || signed_height == i32::MIN
                || !matches!(bits_per_pixel, 24 | 32)
                || !matches!(compression, 0 | 3)
            {
                return None;
            }
            let width = width as usize;
            let height = signed_height.unsigned_abs() as usize;
            let pixel_count = width.checked_mul(height)?;
            if pixel_count == 0 || pixel_count > MAX_PIXELS {
                return None;
            }
            let bytes_per_pixel = (bits_per_pixel / 8) as usize;
            let row_stride = width.checked_mul(bytes_per_pixel)?.checked_add(3)? & !3;
            let image_size = row_stride.checked_mul(height)?;
            let data_offset = if compression == 3 && header_size == 40 {
                header_size.checked_add(12)?
            } else {
                header_size
            };
            if data_offset.checked_add(image_size)? > size {
                return None;
            }
            let top_down = signed_height < 0;
            let mut rgba = vec![0; pixel_count * 4];
            for y in 0..height {
                let source_y = if top_down { y } else { height - 1 - y };
                let row = data_offset + source_y * row_stride;
                for x in 0..width {
                    let src = row + x * bytes_per_pixel;
                    let dst = (y * width + x) * 4;
                    rgba[dst..dst + 4].copy_from_slice(&[
                        bytes[src + 2],
                        bytes[src + 1],
                        bytes[src],
                        if bits_per_pixel == 32 {
                            bytes[src + 3]
                        } else {
                            255
                        },
                    ]);
                }
            }
            Some((width as u32, height as u32, rgba))
        })();
        let _ = GlobalUnlock(memory);
        Ok(parsed)
    })();
    let _ = unsafe { CloseClipboard() };
    result
}

/// Optional Wintab backend loaded from the tablet driver's `wintab32.dll`.
/// The driver API is resolved at runtime so systems without a Wintab driver
/// continue to use Windows Ink or the normal window input path.
#[cfg(target_os = "windows")]
mod wintab {
    use super::{Arc, ArrayQueue, PenPacket};
    use std::ffi::{c_char, c_void};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::ScreenToClient,
        UI::{
            HiDpi::GetDpiForWindow,
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        },
    };

    const WTI_DEFSYSCTX: u32 = 4;
    const WTI_DEVICES: u32 = 100;
    const DVC_PKTDATA: u32 = 6;
    const DVC_NPRESSURE: u32 = 15;
    const DVC_ORIENTATION: u32 = 17;
    const CXO_SYSTEM: u32 = 0x0001;
    const CXO_MESSAGES: u32 = 0x0004;
    const PK_TIME: u32 = 0x0004;
    const PK_BUTTONS: u32 = 0x0040;
    const PK_X: u32 = 0x0080;
    const PK_Y: u32 = 0x0100;
    const PK_NORMAL_PRESSURE: u32 = 0x0400;
    const PK_ORIENTATION: u32 = 0x1000;
    const WT_PACKET_OFFSET: u32 = 0;
    const SUBCLASS_ID: usize = 0x454655445746;
    const PACKET_WORDS: usize = 16;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct LogContextA {
        name: [c_char; 40],
        options: u32,
        status: u32,
        locks: u32,
        message_base: u32,
        device: u32,
        packet_rate: u32,
        packet_data: u32,
        packet_mode: u32,
        move_mask: u32,
        button_down_mask: u32,
        button_up_mask: u32,
        input_origin: [i32; 3],
        input_extent: [i32; 3],
        output_origin: [i32; 3],
        output_extent: [i32; 3],
        sensitivity: [i32; 3],
        system_mode: i32,
        system_origin: [i32; 2],
        system_extent: [i32; 2],
        system_sensitivity: [i32; 2],
    }
    impl Default for LogContextA {
        fn default() -> Self {
            unsafe { std::mem::zeroed() }
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Axis {
        min: i32,
        max: i32,
        units: u32,
        resolution: i32,
    }

    type WtInfoA = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;
    type WtOpenA = unsafe extern "system" fn(HWND, *mut LogContextA, i32) -> *mut c_void;
    type WtPacket = unsafe extern "system" fn(*mut c_void, u32, *mut u32) -> i32;
    type WtClose = unsafe extern "system" fn(*mut c_void) -> i32;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
    }

    unsafe fn load_proc<T: Copy>(module: *mut c_void, name: &'static [u8]) -> Option<T> {
        let address = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
        if address.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute_copy(&address) })
        }
    }

    #[derive(Clone, Copy, Default)]
    struct AxisRange {
        min: i32,
        max: i32,
    }

    struct HookState {
        queue: Arc<ArrayQueue<PenPacket>>,
        module: *mut c_void,
        context: *mut c_void,
        packet_fn: WtPacket,
        close_fn: WtClose,
        message: u32,
        packet_mask: u32,
        pressure_range: AxisRange,
        /// Largest raw pressure seen, to correct a wrong reported range.
        pressure_seen_max: std::cell::Cell<i32>,
        orientation: [AxisRange; 3],
    }

    fn field_offset(mask: u32, target: u32) -> Option<usize> {
        let mut offset = 0;
        for bit_index in 0..32 {
            let bit = 1u32 << bit_index;
            if mask & bit == 0 {
                continue;
            }
            if bit == target {
                return Some(offset);
            }
            let size = match bit {
                PK_TIME | PK_BUTTONS | PK_X | PK_Y | PK_NORMAL_PRESSURE => 4,
                PK_ORIENTATION => 12,
                _ => return None,
            };
            offset += size;
        }
        None
    }

    fn packet_u32(words: &[u32; PACKET_WORDS], offset: usize) -> u32 {
        words[offset / 4]
    }
    fn packet_i32(words: &[u32; PACKET_WORDS], offset: usize) -> i32 {
        words[offset / 4] as i32
    }
    fn normalized(value: i32, range: AxisRange) -> f32 {
        let span = range.max.saturating_sub(range.min);
        if span <= 0 {
            0.0
        } else {
            (value.saturating_sub(range.min) as f32 / span as f32).clamp(0.0, 1.0)
        }
    }

    fn push_packet(hwnd: HWND, state: &HookState, serial: u32) {
        let mut words = [0u32; PACKET_WORDS];
        if unsafe { (state.packet_fn)(state.context, serial, words.as_mut_ptr()) } == 0 {
            return;
        }
        let (Some(time_offset), Some(x_offset), Some(y_offset)) = (
            field_offset(state.packet_mask, PK_TIME),
            field_offset(state.packet_mask, PK_X),
            field_offset(state.packet_mask, PK_Y),
        ) else {
            return;
        };
        let mut screen = windows::Win32::Foundation::POINT {
            x: packet_i32(&words, x_offset),
            y: packet_i32(&words, y_offset),
        };
        if !unsafe { ScreenToClient(hwnd, &mut screen) }.as_bool() {
            return;
        }
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(1) as f32 / 96.0;
        let pressure = field_offset(state.packet_mask, PK_NORMAL_PRESSURE)
            .map(|offset| {
                let raw = packet_u32(&words, offset) as i32;
                if raw > state.pressure_seen_max.get() {
                    state.pressure_seen_max.set(raw);
                }
                let range = AxisRange {
                    min: state.pressure_range.min,
                    max: state.pressure_range.max.max(state.pressure_seen_max.get()),
                };
                normalized(raw, range)
            })
            .unwrap_or(1.0);
        let (tilt_x, tilt_y, rotation) =
            if let Some(offset) = field_offset(state.packet_mask, PK_ORIENTATION) {
                let azimuth = normalized(packet_i32(&words, offset), state.orientation[0]);
                let altitude = normalized(packet_i32(&words, offset + 4), state.orientation[1]);
                let theta = azimuth * std::f32::consts::TAU;
                // Wintab altitude is 0 at the tablet plane and 90 degrees
                // when the pen is perpendicular to it. Windows Ink expresses
                // tilt as degrees away from perpendicular, so convert the
                // altitude to the same 0..1 tilt magnitude here.
                let altitude_radians = altitude * std::f32::consts::FRAC_PI_2;
                let tilt = altitude_radians.cos().max(0.0);
                let twist = normalized(packet_i32(&words, offset + 8), state.orientation[2]);
                (
                    theta.cos() * tilt,
                    theta.sin() * tilt,
                    twist * std::f32::consts::TAU,
                )
            } else {
                (0.0, 0.0, 0.0)
            };
        let _buttons =
            field_offset(state.packet_mask, PK_BUTTONS).map(|offset| packet_u32(&words, offset));
        let packet = PenPacket {
            x: screen.x as f32 / dpi,
            y: screen.y as f32 / dpi,
            pressure,
            tilt_x,
            tilt_y,
            rotation,
            time_ms: packet_u32(&words, time_offset) as u64,
            received_at: std::time::Instant::now(),
        };
        if state.queue.push(packet).is_err() {
            let _ = state.queue.pop();
            let _ = state.queue.push(packet);
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        data: usize,
    ) -> LRESULT {
        let state = unsafe { &*(data as *const HookState) };
        if msg == state.message && lparam.0 as *mut c_void == state.context {
            push_packet(hwnd, state, wparam.0 as u32);
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    pub struct WintabHook {
        hwnd: HWND,
        state: Box<HookState>,
    }

    impl WintabHook {
        pub fn install(
            hwnd: *mut c_void,
            queue: Arc<ArrayQueue<PenPacket>>,
        ) -> Result<Self, String> {
            let module = unsafe { LoadLibraryA(c"wintab32.dll".as_ptr()) };
            if module.is_null() {
                return Err("Wintab driver (wintab32.dll) is unavailable".into());
            }
            let result = (|| unsafe {
                let wt_info: WtInfoA = load_proc(module, b"WTInfoA\0")
                    .ok_or_else(|| "Wintab WTInfoA export is unavailable".to_string())?;
                let wt_open: WtOpenA = load_proc(module, b"WTOpenA\0")
                    .ok_or_else(|| "Wintab WTOpenA export is unavailable".to_string())?;
                let packet_fn: WtPacket = load_proc(module, b"WTPacket\0")
                    .ok_or_else(|| "Wintab WTPacket export is unavailable".to_string())?;
                let close_fn: WtClose = load_proc(module, b"WTClose\0")
                    .ok_or_else(|| "Wintab WTClose export is unavailable".to_string())?;
                if wt_info(0, 0, std::ptr::null_mut()) == 0 {
                    return Err("Wintab driver did not respond to WTInfo".into());
                }
                let mut context = LogContextA::default();
                if wt_info(WTI_DEFSYSCTX, 0, (&mut context as *mut LogContextA).cast()) == 0 {
                    return Err("Wintab system context is unavailable".into());
                }
                let device_category = WTI_DEVICES.saturating_add(if context.device == u32::MAX {
                    0
                } else {
                    context.device
                });
                let mut device_packet_mask = 0u32;
                let _ = wt_info(
                    device_category,
                    DVC_PKTDATA,
                    (&mut device_packet_mask as *mut u32).cast(),
                );
                let mut pressure_axis = Axis::default();
                let pressure_supported = wt_info(
                    device_category,
                    DVC_NPRESSURE,
                    (&mut pressure_axis as *mut Axis).cast(),
                ) != 0;
                let mut orientation_axes = [Axis::default(); 3];
                let orientation_supported = wt_info(
                    device_category,
                    DVC_ORIENTATION,
                    orientation_axes.as_mut_ptr().cast(),
                ) != 0;

                // Always ask for pressure. Some drivers leave it out of the
                // DVC_PKTDATA mask even though they deliver it, and without
                // it every packet would read as full pressure.
                let _ = device_packet_mask;
                let mut packet_mask = PK_TIME | PK_BUTTONS | PK_X | PK_Y | PK_NORMAL_PRESSURE;
                if orientation_supported {
                    packet_mask |= PK_ORIENTATION;
                }
                context.options |= CXO_SYSTEM | CXO_MESSAGES;
                // Report positions in virtual-screen pixels with the origin at
                // the top left. The default system context may report tablet
                // counts (and Wintab's Y axis points up), which would place
                // every packet outside the canvas and silently fall back to
                // pressure-less window input.
                context.output_origin[0] = context.system_origin[0];
                context.output_origin[1] = context.system_origin[1];
                context.output_extent[0] = context.system_extent[0];
                context.output_extent[1] = -context.system_extent[1];
                context.packet_data = packet_mask;
                context.packet_mode = 0;
                context.move_mask =
                    packet_mask & (PK_X | PK_Y | PK_NORMAL_PRESSURE | PK_ORIENTATION);
                context.name[..6].copy_from_slice(&b"Efude\0".map(|byte| byte as c_char));
                let hwnd = HWND(hwnd);
                let context_handle = wt_open(hwnd, &mut context, 1);
                if context_handle.is_null() {
                    return Err("Wintab could not open a tablet context".into());
                }
                let state = Box::new(HookState {
                    queue,
                    module,
                    context: context_handle,
                    packet_fn,
                    close_fn,
                    message: context.message_base + WT_PACKET_OFFSET,
                    packet_mask,
                    pressure_range: if pressure_supported && pressure_axis.max > pressure_axis.min {
                        AxisRange {
                            min: pressure_axis.min,
                            max: pressure_axis.max,
                        }
                    } else {
                        // Unknown range: assume the common 1024 levels;
                        // `push_packet` widens it if larger values appear.
                        AxisRange { min: 0, max: 1023 }
                    },
                    pressure_seen_max: std::cell::Cell::new(0),
                    orientation: orientation_axes.map(|axis| AxisRange {
                        min: axis.min,
                        max: axis.max,
                    }),
                });
                let data = (&*state as *const HookState) as usize;
                if !SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID, data).as_bool() {
                    let _ = (state.close_fn)(state.context);
                    return Err("Could not attach Wintab messages to the application window".into());
                }
                Ok(Self { hwnd, state })
            })();
            if result.is_err() {
                unsafe { FreeLibrary(module) };
            }
            result
        }
    }

    impl Drop for WintabHook {
        fn drop(&mut self) {
            unsafe {
                let _ = RemoveWindowSubclass(self.hwnd, Some(window_proc), SUBCLASS_ID);
                let _ = (self.state.close_fn)(self.state.context);
                let _ = FreeLibrary(self.state.module);
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use wintab::WintabHook;
#[derive(Default)]
pub struct InputNormalizer {
    started_at: Option<std::time::Instant>,
}
impl InputNormalizer {
    pub fn begin(&mut self) {
        self.started_at = Some(std::time::Instant::now());
    }
    pub fn point(&self, x: f32, y: f32, pressure: f32) -> InkPoint {
        InkPoint::new(
            x,
            y,
            pressure,
            self.started_at
                .map_or(0, |t| t.elapsed().as_millis() as u64),
        )
    }
    /// Build a complete pen point for platform backends that expose tilt and barrel rotation.
    pub fn pen_point(
        &self,
        x: f32,
        y: f32,
        pressure: f32,
        tilt_x: f32,
        tilt_y: f32,
        rotation_radians: f32,
    ) -> InkPoint {
        let mut point = self.point(x, y, pressure);
        point.tilt = glam::Vec2::new(tilt_x.clamp(-1., 1.), tilt_y.clamp(-1., 1.));
        point.rotation = rotation_radians;
        point
    }
}

pub struct InputQueue {
    pub sender: SyncSender<InkPoint>,
    pub receiver: Receiver<InkPoint>,
}
impl InputQueue {
    pub fn bounded(capacity: usize) -> Self {
        let (sender, receiver) = sync_channel(capacity.max(1));
        Self { sender, receiver }
    }
}

#[cfg(target_os = "windows")]
pub fn pointer_pressure(raw_pressure: u32) -> f32 {
    (raw_pressure as f32 / 1024.).clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_stroke_replays_all_input_channels() {
        let mut original = InkPoint::new(12.5, 30.0, 0.42, 14);
        original.tilt = glam::Vec2::new(-0.3, 0.7);
        original.rotation = 1.2;
        let mut log = StrokeLog::default();
        log.push(&[original]);
        let replayed = log.replay(0).unwrap();
        assert_eq!(replayed, vec![original]);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordedPoint {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    #[serde(default)]
    pub rotation: f32,
    pub time_ms: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StrokeLog {
    pub version: u32,
    pub strokes: Vec<Vec<RecordedPoint>>,
}
impl StrokeLog {
    pub fn push(&mut self, points: &[InkPoint]) {
        self.strokes.push(
            points
                .iter()
                .map(|p| RecordedPoint {
                    x: p.position.x,
                    y: p.position.y,
                    pressure: p.pressure,
                    tilt_x: p.tilt.x,
                    tilt_y: p.tilt.y,
                    rotation: p.rotation,
                    time_ms: p.time_ms,
                })
                .collect(),
        );
        if self.strokes.len() > 128 {
            self.strokes.remove(0);
        }
        self.version = 2;
    }
    pub fn replay(&self, index: usize) -> Option<Vec<InkPoint>> {
        self.strokes.get(index).map(|stroke| {
            stroke
                .iter()
                .map(|p| InkPoint {
                    position: glam::Vec2::new(p.x, p.y),
                    pressure: p.pressure,
                    taper: 1.0,
                    tilt: glam::Vec2::new(p.tilt_x, p.tilt_y),
                    rotation: p.rotation,
                    time_ms: p.time_ms,
                })
                .collect()
        })
    }
    pub fn save(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
    pub fn load(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
}
#[cfg(not(target_os = "windows"))]
pub fn pointer_pressure(raw_pressure: u32) -> f32 {
    (raw_pressure as f32 / 1024.).clamp(0., 1.)
}
