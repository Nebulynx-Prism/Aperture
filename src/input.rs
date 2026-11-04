//! Input injection module for OpenReality.
//!
//! Handles injecting keyboard, mouse, scroll, touch, and gamepad input on the
//! host PC based on events received from the connected VR headset. Mirrors the
//! `IInputInjection` interface found in Virtual Desktop Streamer's
//! `Shell.Native.dll`.
//!
//! # Safety
//! This module makes extensive use of `unsafe` blocks to call Win32 APIs
//! (`SendInput`, cursor and pointer injection functions). All unsafe calls are
//! individually scoped and documented.

use anyhow::{Context, Result};
use std::mem;
use tracing::{debug, error, info, trace, warn};
use windows::Win32::Foundation::{BOOL, HANDLE, POINT, TRUE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, GetIconInfo, GetSystemMetrics, CURSORINFO, CURSOR_SHOWING, ICONINFO,
    SM_CXSCREEN, SM_CYSCREEN,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Keyboard input event received from the VR headset.
#[derive(Debug, Clone, Copy)]
pub struct KeyboardInput {
    /// Win32 virtual-key code (e.g. `VK_RETURN`).
    pub virtual_key: u16,
    /// Hardware scan code.
    pub scan_code: u16,
    /// `true` when this is a key-release event.
    pub key_up: bool,
    /// `true` for extended keys (e.g. right Ctrl/Alt, arrow keys).
    pub extended: bool,
}

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// First X button (typically "Back").
    X1,
    /// Second X button (typically "Forward").
    X2,
}

/// Gamepad model for the virtual controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamepadModel {
    /// Standard XInput-compatible Xbox 360 controller.
    Xbox360,
    /// DualShock 4 controller.
    DualShock4,
}

/// XInput-compatible gamepad state received from the VR headset.
#[derive(Debug, Clone, Copy, Default)]
pub struct GamepadState {
    /// XINPUT_GAMEPAD button flags.
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub left_thumb_x: i16,
    pub left_thumb_y: i16,
    pub right_thumb_x: i16,
    pub right_thumb_y: i16,
}

/// Information about the current cursor shape and position.
#[derive(Debug, Clone)]
pub struct CursorInfo {
    /// Screen X position.
    pub x: i32,
    /// Screen Y position.
    pub y: i32,
    /// Cursor bitmap width in pixels.
    pub width: u32,
    /// Cursor bitmap height in pixels.
    pub height: u32,
    /// Hotspot X offset within the bitmap.
    pub hotspot_x: u32,
    /// Hotspot Y offset within the bitmap.
    pub hotspot_y: u32,
    /// BGRA pixel data of the cursor image.
    pub bitmap: Vec<u8>,
    /// Whether the cursor is currently visible.
    pub visible: bool,
}

// ---------------------------------------------------------------------------
// InputInjector — keyboard, mouse, scroll, touch
// ---------------------------------------------------------------------------

/// Injects HID input events into the desktop session on behalf of a remote VR
/// headset. Wraps Win32 `SendInput` and pointer-injection APIs.
pub struct InputInjector {
    /// Set of virtual-key codes currently held down so we can release them all.
    held_keys: Vec<u16>,
    /// Set of mouse buttons currently held down.
    held_buttons: Vec<MouseButton>,
}

impl InputInjector {
    /// Create a new `InputInjector`.
    pub fn new() -> Result<Self> {
        info!("InputInjector initialised");
        Ok(Self {
            held_keys: Vec::new(),
            held_buttons: Vec::new(),
        })
    }

    // -- Keyboard -----------------------------------------------------------

    /// Inject a single keyboard event via `SendInput`.
    pub fn inject_keyboard(&mut self, input: &KeyboardInput) -> Result<()> {
        trace!(
            vk = input.virtual_key,
            scan = input.scan_code,
            up = input.key_up,
            ext = input.extended,
            "inject_keyboard"
        );

        let mut flags = KEYEVENTF_SCANCODE;
        if input.key_up {
            flags |= KEYEVENTF_KEYUP;
        }
        if input.extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }

        let ki = KEYBDINPUT {
            wVk: VIRTUAL_KEY(input.virtual_key),
            wScan: input.scan_code,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inp = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki },
        };

        // SAFETY: We pass a valid INPUT struct to SendInput. The pointer is to
        // a local variable on the stack with correct size.
        let sent = unsafe { SendInput(&mut [inp], mem::size_of::<INPUT>() as i32) };
        if sent == 0 {
            anyhow::bail!("SendInput (keyboard) returned 0 — UIPI may be blocking injection");
        }

        // Track held keys for `release_all`.
        if input.key_up {
            self.held_keys.retain(|&k| k != input.virtual_key);
        } else if !self.held_keys.contains(&input.virtual_key) {
            self.held_keys.push(input.virtual_key);
        }

        Ok(())
    }

    // -- Mouse move ---------------------------------------------------------

    /// Inject a mouse-move event.
    ///
    /// * `x`, `y` — coordinates. If `absolute` is true these are normalised
    ///   screen coordinates in `0.0..=1.0`; otherwise they are relative deltas.
    /// * `absolute` — whether to move to an absolute screen position.
    pub fn inject_mouse_move(&self, x: f64, y: f64, absolute: bool) -> Result<()> {
        trace!(x, y, absolute, "inject_mouse_move");

        let mut flags = MOUSEEVENTF_MOVE;
        let (dx, dy) = if absolute {
            flags |= MOUSEEVENTF_ABSOLUTE;
            // SendInput absolute coordinates are in the range 0..65535.
            let ax = (x.clamp(0.0, 1.0) * 65535.0) as i32;
            let ay = (y.clamp(0.0, 1.0) * 65535.0) as i32;
            (ax, ay)
        } else {
            (x as i32, y as i32)
        };

        let mi = MOUSEINPUT {
            dx,
            dy,
            mouseData: 0,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inp = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi },
        };

        // SAFETY: Valid INPUT struct passed to SendInput.
        let sent = unsafe { SendInput(&mut [inp], mem::size_of::<INPUT>() as i32) };
        if sent == 0 {
            anyhow::bail!("SendInput (mouse move) returned 0");
        }
        Ok(())
    }

    // -- Mouse button -------------------------------------------------------

    /// Inject a mouse button press or release.
    pub fn inject_mouse_button(&mut self, button: MouseButton, pressed: bool) -> Result<()> {
        trace!(?button, pressed, "inject_mouse_button");

        let (flags, mouse_data) = match (button, pressed) {
            (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0u32),
            (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::X1, true) => (MOUSEEVENTF_XDOWN, 0x0001),  // XBUTTON1
            (MouseButton::X1, false) => (MOUSEEVENTF_XUP, 0x0001),
            (MouseButton::X2, true) => (MOUSEEVENTF_XDOWN, 0x0002),  // XBUTTON2
            (MouseButton::X2, false) => (MOUSEEVENTF_XUP, 0x0002),
        };

        let mi = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: mouse_data,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inp = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi },
        };

        // SAFETY: Valid INPUT struct.
        let sent = unsafe { SendInput(&mut [inp], mem::size_of::<INPUT>() as i32) };
        if sent == 0 {
            anyhow::bail!("SendInput (mouse button) returned 0");
        }

        // Track held buttons.
        if pressed {
            if !self.held_buttons.contains(&button) {
                self.held_buttons.push(button);
            }
        } else {
            self.held_buttons.retain(|&b| b != button);
        }

        Ok(())
    }

    // -- Scroll -------------------------------------------------------------

    /// Inject a scroll event.
    ///
    /// * `delta_x` — horizontal scroll amount (positive = right).
    /// * `delta_y` — vertical scroll amount (positive = up, matches
    ///   `WHEEL_DELTA` convention).
    pub fn inject_scroll(&self, delta_x: i32, delta_y: i32) -> Result<()> {
        trace!(delta_x, delta_y, "inject_scroll");

        // Vertical scroll
        if delta_y != 0 {
            let mi = MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: delta_y as u32,
                dwFlags: MOUSEEVENTF_WHEEL,
                time: 0,
                dwExtraInfo: 0,
            };

            let mut inp = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 { mi },
            };

            // SAFETY: Valid INPUT struct.
            let sent = unsafe { SendInput(&mut [inp], mem::size_of::<INPUT>() as i32) };
            if sent == 0 {
                anyhow::bail!("SendInput (vertical scroll) returned 0");
            }
        }

        // Horizontal scroll
        if delta_x != 0 {
            let mi = MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: delta_x as u32,
                dwFlags: MOUSEEVENTF_HWHEEL,
                time: 0,
                dwExtraInfo: 0,
            };

            let mut inp = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 { mi },
            };

            // SAFETY: Valid INPUT struct.
            let sent = unsafe { SendInput(&mut [inp], mem::size_of::<INPUT>() as i32) };
            if sent == 0 {
                anyhow::bail!("SendInput (horizontal scroll) returned 0");
            }
        }

        Ok(())
    }

    // -- Touch / pointer injection ------------------------------------------

    /// Inject a touch event using `SendInput` mouse emulation.
    ///
    /// A full production implementation would use the Windows Pointer Injection
    /// API (`InitializeTouchInjection` / `InjectTouchInput`) for proper
    /// multi-touch support. This simplified version moves the cursor to the
    /// specified position and synthesises a left-click, which is sufficient for
    /// single-touch interaction.
    ///
    /// * `x`, `y` — normalised screen position in `0.0..=1.0`.
    /// * `id` — touch-point identifier (used for multi-touch tracking).
    /// * `pressed` — `true` for touch-down / move, `false` for touch-up.
    pub fn inject_touch(&self, x: f64, y: f64, id: u32, pressed: bool) -> Result<()> {
        trace!(x, y, id, pressed, "inject_touch (mouse emulation)");

        // Move to the touch position (absolute).
        let ax = (x.clamp(0.0, 1.0) * 65535.0) as i32;
        let ay = (y.clamp(0.0, 1.0) * 65535.0) as i32;

        let button_flag: MOUSE_EVENT_FLAGS = if pressed {
            MOUSEEVENTF_LEFTDOWN
        } else {
            MOUSEEVENTF_LEFTUP
        };

        let mi = MOUSEINPUT {
            dx: ax,
            dy: ay,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | button_flag,
            time: 0,
            dwExtraInfo: 0,
        };

        let mut inp = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi },
        };

        // SAFETY: Valid INPUT struct.
        let sent = unsafe { SendInput(&mut [inp], mem::size_of::<INPUT>() as i32) };
        if sent == 0 {
            anyhow::bail!("SendInput (touch) returned 0");
        }
        Ok(())
    }

    // -- Release all --------------------------------------------------------

    /// Release every key and mouse button that is currently held down.
    ///
    /// This should be called when the VR headset disconnects or when the user
    /// switches away, to prevent keys/buttons from sticking.
    pub fn release_all(&mut self) -> Result<()> {
        info!(
            keys = self.held_keys.len(),
            buttons = self.held_buttons.len(),
            "release_all: releasing held inputs"
        );

        // Release held keys (clone to avoid borrow issues).
        let keys: Vec<u16> = self.held_keys.clone();
        for vk in keys {
            self.inject_keyboard(&KeyboardInput {
                virtual_key: vk,
                scan_code: 0,
                key_up: true,
                extended: false,
            })?;
        }

        // Release held mouse buttons.
        let buttons: Vec<MouseButton> = self.held_buttons.clone();
        for btn in buttons {
            self.inject_mouse_button(btn, false)?;
        }

        debug_assert!(self.held_keys.is_empty());
        debug_assert!(self.held_buttons.is_empty());
        Ok(())
    }
}

impl Drop for InputInjector {
    fn drop(&mut self) {
        if !self.held_keys.is_empty() || !self.held_buttons.is_empty() {
            warn!("InputInjector dropped with held inputs — releasing");
            if let Err(e) = self.release_all() {
                error!("Failed to release inputs on drop: {e:#}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GamepadEmulator — virtual gamepad via ViGEm (stub)
// ---------------------------------------------------------------------------

/// Emulates a virtual gamepad (Xbox 360 or DualShock 4).
///
/// # Note on ViGEm
/// A real implementation of virtual gamepad injection on Windows requires the
/// [ViGEm Bus Driver](https://github.com/nefarius/ViGEmBus) to be installed
/// and uses the
/// [ViGEm Client library](https://github.com/nefarius/ViGEmClient) (C API) or
/// a Rust wrapper such as `vigem-client`. Because ViGEm requires a signed
/// kernel-mode driver that cannot be bundled as a simple crate dependency, this
/// implementation is provided as a **stub** that logs the intended actions.
///
/// To integrate ViGEm in production:
/// 1. Add `vigem-client` to Cargo.toml (or use raw FFI to `ViGEmClient.dll`).
/// 2. Call `vigem_connect` to open a handle to the bus.
/// 3. Call `vigem_target_x360_alloc` / `vigem_target_ds4_alloc` to create a
///    virtual device.
/// 4. Plug it in with `vigem_target_add`.
/// 5. Submit reports with `vigem_target_x360_update` /
///    `vigem_target_ds4_update`.
pub struct GamepadEmulator {
    model: GamepadModel,
    connected: bool,
    last_state: GamepadState,
}

impl GamepadEmulator {
    /// Create and "connect" a new virtual gamepad.
    ///
    /// In a real implementation this would allocate a ViGEm target and plug it
    /// into the virtual bus.
    pub fn new(model: GamepadModel) -> Result<Self> {
        info!(?model, "GamepadEmulator::new — STUB: virtual gamepad created");
        Ok(Self {
            model,
            connected: true,
            last_state: GamepadState::default(),
        })
    }

    /// Submit an updated gamepad state.
    ///
    /// In a real implementation this would call
    /// `vigem_target_x360_update` or `vigem_target_ds4_update`.
    pub fn update_state(&mut self, state: &GamepadState) -> Result<()> {
        if !self.connected {
            anyhow::bail!("GamepadEmulator: gamepad is not connected");
        }

        trace!(
            model = ?self.model,
            buttons = state.buttons,
            lt = state.left_trigger,
            rt = state.right_trigger,
            lx = state.left_thumb_x,
            ly = state.left_thumb_y,
            rx = state.right_thumb_x,
            ry = state.right_thumb_y,
            "GamepadEmulator::update_state — STUB"
        );

        self.last_state = *state;
        Ok(())
    }

    /// Disconnect the virtual gamepad.
    ///
    /// In a real implementation this would call `vigem_target_remove` and free
    /// the target.
    pub fn disconnect(&mut self) -> Result<()> {
        if !self.connected {
            warn!("GamepadEmulator::disconnect called but already disconnected");
            return Ok(());
        }
        info!(model = ?self.model, "GamepadEmulator::disconnect — STUB: virtual gamepad removed");
        self.connected = false;
        Ok(())
    }

    /// Returns the model of this emulator.
    pub fn model(&self) -> GamepadModel {
        self.model
    }

    /// Returns whether the virtual gamepad is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected
    }
}

impl Drop for GamepadEmulator {
    fn drop(&mut self) {
        if self.connected {
            info!("GamepadEmulator dropped while connected — disconnecting");
            if let Err(e) = self.disconnect() {
                error!("Failed to disconnect gamepad on drop: {e:#}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CursorCapture — capture current cursor shape & position
// ---------------------------------------------------------------------------

/// Captures the current system cursor shape and position for streaming to the
/// VR headset so that the remote user sees the real cursor.
pub struct CursorCapture;

impl CursorCapture {
    /// Capture the current cursor information.
    ///
    /// Returns `Ok(None)` if the cursor is hidden or unavailable, and
    /// `Ok(Some(CursorInfo))` with the cursor bitmap data otherwise.
    pub fn capture_cursor() -> Result<Option<CursorInfo>> {
        // Initialise CURSORINFO.
        let mut ci = CURSORINFO {
            cbSize: mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };

        // SAFETY: `ci` is correctly sized via `cbSize`.
        unsafe { GetCursorInfo(&mut ci)? };

        let visible = (ci.flags.0 & CURSOR_SHOWING.0) != 0;
        if !visible {
            return Ok(None);
        }

        let hcursor = ci.hCursor;
        if hcursor.is_invalid() {
            return Ok(None);
        }

        // Get the icon/cursor bitmap information.
        let mut icon_info = ICONINFO::default();
        // SAFETY: `hcursor` is valid; `icon_info` receives the result.
        unsafe { GetIconInfo(hcursor, &mut icon_info)? };

        let hotspot_x = icon_info.xHotspot;
        let hotspot_y = icon_info.yHotspot;

        // We need to read the colour bitmap. If `hbmColor` is null the cursor
        // is monochrome — we still read `hbmMask` in that case.
        let hbm = if !icon_info.hbmColor.is_invalid() {
            icon_info.hbmColor
        } else {
            icon_info.hbmMask
        };

        let bitmap_data = Self::read_bitmap(hbm)?;

        // Clean up GDI objects returned by GetIconInfo.
        // SAFETY: Handles returned by GetIconInfo must be freed by the caller.
        unsafe {
            if !icon_info.hbmColor.is_invalid() {
                let _ = DeleteObject(icon_info.hbmColor);
            }
            if !icon_info.hbmMask.is_invalid() {
                let _ = DeleteObject(icon_info.hbmMask);
            }
        }

        match bitmap_data {
            Some((width, height, pixels)) => Ok(Some(CursorInfo {
                x: ci.ptScreenPos.x,
                y: ci.ptScreenPos.y,
                width,
                height,
                hotspot_x,
                hotspot_y,
                bitmap: pixels,
                visible: true,
            })),
            None => Ok(None),
        }
    }

    /// Read BGRA pixel data from an HBITMAP.
    fn read_bitmap(hbm: HBITMAP) -> Result<Option<(u32, u32, Vec<u8>)>> {
        // First call GetDIBits with a null buffer to retrieve dimensions.
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: mem::size_of::<BITMAPINFOHEADER>() as u32,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0 as u32,
                ..Default::default()
            },
            ..Default::default()
        };

        // SAFETY: Create a temporary compatible DC for reading the bitmap.
        let hdc = unsafe { CreateCompatibleDC(None) };
        if hdc.is_invalid() {
            anyhow::bail!("CreateCompatibleDC failed");
        }

        // SAFETY: Select the bitmap into the DC so GetDIBits can read it.
        let old_bm = unsafe { SelectObject(hdc, hbm) };

        // Query dimensions.
        // SAFETY: First call with null buffer to fill bmiHeader.
        let scanlines = unsafe {
            GetDIBits(hdc, hbm, 0, 0, None, &mut bmi, DIB_RGB_COLORS)
        };
        if scanlines == 0 && bmi.bmiHeader.biWidth == 0 {
            // Clean up and return None.
            unsafe {
                SelectObject(hdc, old_bm);
                let _ = DeleteDC(hdc);
            }
            return Ok(None);
        }

        let width = bmi.bmiHeader.biWidth as u32;
        let height = bmi.bmiHeader.biHeight.unsigned_abs();
        let stride = ((width * 32 + 31) / 32) * 4; // bytes per row, 32bpp
        let buf_size = (stride * height) as usize;
        let mut buffer = vec![0u8; buf_size];

        // Force top-down DIB so the rows come in the expected order.
        bmi.bmiHeader.biHeight = -(height as i32);
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0 as u32;

        // SAFETY: buffer is correctly sized.
        let result = unsafe {
            GetDIBits(
                hdc,
                hbm,
                0,
                height,
                Some(buffer.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };

        // Clean up.
        unsafe {
            SelectObject(hdc, old_bm);
            let _ = DeleteDC(hdc);
        }

        if result == 0 {
            anyhow::bail!("GetDIBits failed to read cursor bitmap pixels");
        }

        Ok(Some((width, height, buffer)))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_input_round_trip() {
        let ki = KeyboardInput {
            virtual_key: 0x41, // 'A'
            scan_code: 0x1E,
            key_up: false,
            extended: false,
        };
        assert_eq!(ki.virtual_key, 0x41);
        assert!(!ki.key_up);
    }

    #[test]
    fn gamepad_state_default() {
        let gs = GamepadState::default();
        assert_eq!(gs.buttons, 0);
        assert_eq!(gs.left_trigger, 0);
        assert_eq!(gs.right_trigger, 0);
    }

    #[test]
    fn gamepad_emulator_lifecycle() {
        let mut emu = GamepadEmulator::new(GamepadModel::Xbox360).unwrap();
        assert!(emu.is_connected());
        assert_eq!(emu.model(), GamepadModel::Xbox360);

        let state = GamepadState {
            buttons: 0x0001,
            left_trigger: 128,
            ..Default::default()
        };
        emu.update_state(&state).unwrap();
        emu.disconnect().unwrap();
        assert!(!emu.is_connected());

        // Updating after disconnect should fail.
        assert!(emu.update_state(&state).is_err());
    }

    #[test]
    fn mouse_button_enum_equality() {
        assert_eq!(MouseButton::Left, MouseButton::Left);
        assert_ne!(MouseButton::Left, MouseButton::Right);
    }

    #[test]
    fn input_injector_creation() {
        let injector = InputInjector::new();
        assert!(injector.is_ok());
    }
}
