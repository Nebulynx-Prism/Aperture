//! Cursor overlay for tightly packed, top-down BGRA8 primary-monitor frames.
//! Call composite, encode, then restore even when encoding fails. Coordinates are
//! physical desktop pixels; primary monitor origin is (0, 0). No DPI/config writes.
use anyhow::{ensure, Context, Result};
use std::{ffi::c_void, mem::size_of};
use windows::Win32::{
    Foundation::{HANDLE, POINT},
    Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetObjectW,
        SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
        HGDIOBJ,
    },
    UI::WindowsAndMessaging::{
        DrawIconEx, GetCursorInfo, GetIconInfo, GetPhysicalCursorPos, CURSORINFO, CURSOR_SHOWING,
        DI_NORMAL, HICON, ICONINFO,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Clip {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    sx: usize,
    sy: usize,
}

fn clipped(x: i64, y: i64, cw: u32, ch: u32, width: u32, height: u32) -> Option<Clip> {
    let left = x.max(0);
    let top = y.max(0);
    let right = (x + i64::from(cw)).min(i64::from(width));
    let bottom = (y + i64::from(ch)).min(i64::from(height));
    if right <= left || bottom <= top {
        return None;
    }
    Some(Clip {
        x: left as usize,
        y: top as usize,
        w: (right - left) as usize,
        h: (bottom - top) as usize,
        sx: (left - x) as usize,
        sy: (top - y) as usize,
    })
}

fn check_frame(data: &[u8], width: u32, height: u32) -> Result<()> {
    let required = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    ensure!(
        width > 0 && height > 0 && required == Some(data.len()),
        "cursor overlay requires tightly packed BGRA8 pixels matching frame dimensions"
    );
    Ok(())
}

struct IconBitmaps(ICONINFO);
impl Drop for IconBitmaps {
    fn drop(&mut self) {
        unsafe {
            if !self.0.hbmColor.is_invalid() {
                let _ = DeleteObject(self.0.hbmColor);
            }
            if !self.0.hbmMask.is_invalid() {
                let _ = DeleteObject(self.0.hbmMask);
            }
        }
    }
}

// The DC is local to a composite call, never retained across async suspension or
// thread migration. Restore its original selected object before deleting the DC.
struct Dc {
    handle: HDC,
    old: Option<HGDIOBJ>,
}
impl Dc {
    fn new() -> Result<Self> {
        let handle = unsafe { CreateCompatibleDC(None) };
        ensure!(!handle.is_invalid(), "CreateCompatibleDC failed");
        Ok(Self { handle, old: None })
    }
    fn select(&mut self, bitmap: HBITMAP) -> Result<()> {
        let old = unsafe { SelectObject(self.handle, bitmap) };
        ensure!(
            !old.is_invalid() && old.0 as isize != -1,
            "SelectObject failed"
        );
        self.old = Some(old);
        Ok(())
    }
}
impl Drop for Dc {
    fn drop(&mut self) {
        unsafe {
            if let Some(old) = self.old {
                SelectObject(self.handle, old);
            }
            let _ = DeleteDC(self.handle);
        }
    }
}

struct Dib {
    bitmap: HBITMAP,
    bits: *mut u8,
    width: u32,
    height: u32,
}
impl Dib {
    fn new(width: u32, height: u32) -> Result<Self> {
        // Bound allocations for malformed/unexpected cursor dimensions.
        ensure!(
            width > 0 && height > 0 && width <= 1024 && height <= 1024,
            "unsupported cursor dimensions {width}x{height}"
        );
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0)
        }
        .context("CreateDIBSection for cursor failed")?;
        let result = Self {
            bitmap,
            bits: bits.cast(),
            width,
            height,
        };
        ensure!(!result.bits.is_null(), "cursor DIB has no pixels");
        Ok(result)
    }
}
impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.bitmap);
        }
    }
}

struct Saved {
    clip: Clip,
    width: u32,
    height: u32,
}

/// Reuses a cursor-sized DIB and a clean clipped-rectangle backup, never a frame.
/// Overlay only frames captured without an already baked-in OS cursor.
#[derive(Default)]
pub struct CursorOverlay {
    dib: Option<Dib>,
    backup: Vec<u8>,
    saved: Option<Saved>,
}

// A bitmap may be transferred between threads when not selected into a DC.
// Every composite call flushes GDI and destroys its local DC before returning;
// exclusive &mut self prevents concurrent bitmap access. No thread-owned DC is retained.
unsafe impl Send for CursorOverlay {}

impl CursorOverlay {
    pub fn new() -> Result<Self> {
        Ok(Self::default())
    }

    /// Draw on a clean BGRA8 primary-monitor frame. Returns false when hidden/offscreen.
    /// Call restore on this same frame before reusing/replacing its clean pixels.
    pub fn composite(&mut self, data: &mut [u8], width: u32, height: u32) -> Result<bool> {
        check_frame(data, width, height)?;
        ensure!(
            self.saved.is_none(),
            "restore the previous cursor overlay before compositing again"
        );
        let mut cursor = CURSORINFO {
            cbSize: size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetCursorInfo(&mut cursor) }.context("GetCursorInfo failed")?;
        if cursor.flags.0 & CURSOR_SHOWING.0 == 0 || cursor.hCursor.is_invalid() {
            return Ok(false);
        }
        let mut position = POINT::default();
        unsafe { GetPhysicalCursorPos(&mut position) }.context("GetPhysicalCursorPos failed")?;
        let mut info = IconBitmaps(ICONINFO::default());
        let icon = HICON(cursor.hCursor.0); // Borrowed shared cursor; never destroy it.
        unsafe { GetIconInfo(icon, &mut info.0) }.context("GetIconInfo failed")?;
        let color = !info.0.hbmColor.is_invalid();
        let bitmap = if color {
            info.0.hbmColor
        } else {
            info.0.hbmMask
        };
        let mut dimensions = BITMAP::default();
        let got = unsafe {
            GetObjectW(
                bitmap,
                size_of::<BITMAP>() as i32,
                Some((&mut dimensions as *mut BITMAP).cast()),
            )
        };
        ensure!(
            got == size_of::<BITMAP>() as i32 && dimensions.bmWidth > 0 && dimensions.bmHeight > 0,
            "could not read cursor dimensions"
        );
        let cw = dimensions.bmWidth as u32;
        // Monochrome masks stack AND and XOR planes vertically.
        let ch = dimensions.bmHeight as u32 / if color { 1 } else { 2 };
        let Some(clip) = clipped(
            i64::from(position.x) - i64::from(info.0.xHotspot),
            i64::from(position.y) - i64::from(info.0.yHotspot),
            cw,
            ch,
            width,
            height,
        ) else {
            return Ok(false);
        };
        if self.dib.as_ref().map(|d| (d.width, d.height)) != Some((cw, ch)) {
            self.dib = Some(Dib::new(cw, ch)?);
        }
        let dib = self.dib.as_mut().unwrap();
        let mut dc = Dc::new()?;
        dc.select(dib.bitmap)?;
        {
            let pixels =
                unsafe { std::slice::from_raw_parts_mut(dib.bits, cw as usize * ch as usize * 4) };
            pixels.fill(0);
            self.backup.resize(clip.w * clip.h * 4, 0);
            for row in 0..clip.h {
                let src = ((clip.y + row) * width as usize + clip.x) * 4;
                let dst = ((clip.sy + row) * cw as usize + clip.sx) * 4;
                let n = clip.w * 4;
                self.backup[row * n..(row + 1) * n].copy_from_slice(&data[src..src + n]);
                pixels[dst..dst + n].copy_from_slice(&data[src..src + n]);
            }
        }
        // Seeded background makes DI_NORMAL implement alpha AND monochrome XOR
        // against the actual desktop crop rather than against a transparent image.
        let drawn = unsafe {
            DrawIconEx(
                dc.handle, 0, 0, icon, cw as i32, ch as i32, 0, None, DI_NORMAL,
            )
        };
        let flushed = unsafe { GdiFlush() }.as_bool();
        drawn.context("DrawIconEx failed")?;
        ensure!(flushed, "GdiFlush for cursor failed");
        let pixels = unsafe { std::slice::from_raw_parts(dib.bits, cw as usize * ch as usize * 4) };
        for row in 0..clip.h {
            let dst = ((clip.y + row) * width as usize + clip.x) * 4;
            let src = ((clip.sy + row) * cw as usize + clip.sx) * 4;
            let n = clip.w * 4;
            data[dst..dst + n].copy_from_slice(&pixels[src..src + n]);
            // GDI may alter/clear the reserved alpha byte; preserve frame alpha.
            for col in 0..clip.w {
                data[dst + col * 4 + 3] = self.backup[row * n + col * 4 + 3];
            }
        }
        self.saved = Some(Saved {
            clip,
            width,
            height,
        });
        Ok(true)
    }

    /// Restore only the saved clean rectangle after encoding (also on encoder error).
    /// The frame must be the same unmodified buffer/dimensions passed to composite.
    pub fn restore(&mut self, data: &mut [u8], width: u32, height: u32) -> Result<()> {
        let Some(saved) = &self.saved else {
            return Ok(());
        };
        check_frame(data, width, height)?;
        ensure!(
            (saved.width, saved.height) == (width, height),
            "cursor restore frame dimensions changed"
        );
        let c = saved.clip;
        for row in 0..c.h {
            let dst = ((c.y + row) * width as usize + c.x) * 4;
            let n = c.w * 4;
            data[dst..dst + n].copy_from_slice(&self.backup[row * n..(row + 1) * n]);
        }
        self.saved = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipping_hotspot_and_edges() {
        assert_eq!(
            clipped(-2, -1, 5, 4, 10, 10),
            Some(Clip {
                x: 0,
                y: 0,
                w: 3,
                h: 3,
                sx: 2,
                sy: 1
            })
        );
        assert_eq!(
            clipped(8, 9, 5, 4, 10, 10),
            Some(Clip {
                x: 8,
                y: 9,
                w: 2,
                h: 1,
                sx: 0,
                sy: 0
            })
        );
        assert_eq!(clipped(-5, 0, 5, 4, 10, 10), None);
        assert_eq!(clipped(10, 0, 5, 4, 10, 10), None);
    }
    #[test]
    fn restores_only_saved_rectangle_and_reuses_backup() {
        let mut overlay = CursorOverlay::new().unwrap();
        let mut frame = vec![7; 4 * 4 * 4];
        overlay.backup = vec![7; 2 * 2 * 4];
        overlay.saved = Some(Saved {
            clip: Clip {
                x: 1,
                y: 1,
                w: 2,
                h: 2,
                sx: 0,
                sy: 0,
            },
            width: 4,
            height: 4,
        });
        for row in 1..3 {
            frame[(row * 4 + 1) * 4..(row * 4 + 3) * 4].fill(99);
        }
        assert!(overlay.restore(&mut frame, 3, 4).is_err());
        overlay.restore(&mut frame, 4, 4).unwrap();
        assert_eq!(frame, vec![7; 64]);
        assert!(overlay.saved.is_none());
        assert_eq!(overlay.backup.len(), 16);
        overlay.restore(&mut frame, 4, 4).unwrap();
    }
    #[test]
    fn rejects_non_bgra_frame_sizes() {
        assert!(check_frame(&[0; 15], 2, 2).is_err());
        assert!(check_frame(&[], 0, 0).is_err());
        assert!(check_frame(&[0; 16], 2, 2).is_ok());
    }
}
