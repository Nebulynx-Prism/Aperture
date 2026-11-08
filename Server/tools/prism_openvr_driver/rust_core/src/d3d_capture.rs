use crate::{prism_ipc_publish_frame, prism_adapter_rank, PrismFramePose};
use core::ffi::c_void;

#[cfg(windows)]
mod platform {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::CString;
    use std::sync::{Mutex, OnceLock};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;
    use windows::core::Interface;
    use windows::Win32::Foundation::{HANDLE, LUID};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;

    pub type LogFn = Option<extern "C" fn(*const i8)>;
    static LOG_FN: Mutex<LogFn> = Mutex::new(None);
    static FRAME_WIDTH: AtomicU32 = AtomicU32::new(2048);
    static FRAME_HEIGHT: AtomicU32 = AtomicU32::new(1024);

    fn log(message: &str) {
        if let Ok(lock) = LOG_FN.lock() {
            if let Some(callback) = *lock {
                if let Ok(text) = CString::new(message) { callback(text.as_ptr()); }
            }
        }
    }

    fn adapter_list() -> Vec<(u32, DXGI_ADAPTER_DESC1)> {
        unsafe {
            let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return Vec::new() };
            let mut result = Vec::new();
            for index in 0..32 {
                let Ok(adapter) = factory.EnumAdapters1(index) else { break };
                let Ok(desc) = adapter.GetDesc1() else { continue };
                if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 { result.push((index, desc)); }
            }
            result
        }
    }

    fn adapter_rank(desc: &DXGI_ADAPTER_DESC1) -> i32 {
        let end = desc.Description.iter().position(|c| *c == 0).unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..end]);
        prism_adapter_rank(desc.VendorId, name.to_ascii_lowercase().contains("arc"), desc.DedicatedVideoMemory as u64)
    }

    fn luid_u64(luid: LUID) -> u64 {
        (luid.LowPart as u32 as u64) | ((luid.HighPart as u32 as u64) << 32)
    }

    fn preferred_adapter() -> Option<(u32, DXGI_ADAPTER_DESC1)> {
        adapter_list().into_iter().max_by_key(|(_, desc)| adapter_rank(desc))
    }

    fn create_device(index: u32) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
        unsafe {
            let factory = CreateDXGIFactory1::<IDXGIFactory1>()?;
            let adapter = factory.EnumAdapters1(index)?;
            let mut device = None;
            let mut context = None;
            let adapter_base: IDXGIAdapter = adapter.cast()?;
            D3D11CreateDevice(
                Some(&adapter_base), D3D_DRIVER_TYPE_UNKNOWN, None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION,
                Some(&mut device), None, Some(&mut context),
            )?;
            Ok((device.ok_or(windows::core::Error::from_win32())?, context.ok_or(windows::core::Error::from_win32())?))
        }
    }

    struct Slot {
        stage: ID3D11Texture2D,
        ready: ID3D11Query,
        frame: u64,
        pose: PrismFramePose,
        pending: bool,
    }

    struct Texture {
        source: ID3D11Texture2D,
        keyed: IDXGIKeyedMutex,
        slots: [Slot; 3],
        next: usize,
        width: u32,
        height: u32,
        rgba: bool,
    }

    struct Capture {
        device: Option<ID3D11Device>,
        context: Option<ID3D11DeviceContext>,
        adapter: Option<u32>,
        textures: HashMap<u64, Texture>,
        last_frame: Option<u64>,
        last_error: Instant,
    }

    impl Capture {
        fn new() -> Self {
            Self { device: None, context: None, adapter: None, textures: HashMap::new(), last_frame: None, last_error: Instant::now() - std::time::Duration::from_secs(10) }
        }

        fn reset(&mut self) {
            self.textures.clear();
            self.context = None;
            self.device = None;
            self.adapter = None;
        }

        fn report(&mut self, message: &str) {
            if self.last_error.elapsed().as_millis() >= 2000 {
                log(message);
                self.last_error = Instant::now();
            }
        }

        fn open_on(&mut self, index: u32, handle: u64) -> windows::core::Result<Texture> {
            unsafe {
                if self.adapter != Some(index) || self.device.is_none() {
                    self.reset();
                    let (device, context) = create_device(index)?;
                    self.device = Some(device);
                    self.context = Some(context);
                    self.adapter = Some(index);
                }
                let device = self.device.as_ref().unwrap();
                let mut source = None;
                device.OpenSharedResource::<_, ID3D11Texture2D>(HANDLE(handle as usize as *mut c_void), &mut source)?;
                let source = source.ok_or(windows::core::Error::from_win32())?;
                let keyed: IDXGIKeyedMutex = source.cast()?;
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                source.GetDesc(&mut desc);
                let rgba = desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM || desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
                let bgra = desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM || desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB;
                if (!rgba && !bgra) || desc.Width < 512 || desc.Width > 4096 || desc.Width % 2 != 0
                    || desc.Height < 256 || desc.Height > 2048 || desc.SampleDesc.Count != 1
                    || desc.ArraySize != 1 || desc.MipLevels != 1
                { return Err(windows::core::Error::from_win32()); }

                let mut staging = desc;
                staging.Usage = D3D11_USAGE_STAGING;
                staging.BindFlags = 0;
                staging.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                staging.MiscFlags = 0;
                let query_desc = D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 };
                let mut slots = Vec::with_capacity(3);
                for _ in 0..3 {
                    let mut stage = None;
                    let mut ready = None;
                    device.CreateTexture2D(&staging, None, Some(&mut stage))?;
                    device.CreateQuery(&query_desc, Some(&mut ready))?;
                    slots.push(Slot {
                        stage: stage.ok_or(windows::core::Error::from_win32())?,
                        ready: ready.ok_or(windows::core::Error::from_win32())?,
                        frame: 0,
                        pose: PrismFramePose { magic: 0, version: 0, frame_sequence: 0, head: crate::PrismPose { position: [0.0; 3], orientation: [0.0; 4], valid: 0 }, fov: [0.0; 8], sample_time_us: 0 },
                        pending: false,
                    });
                }
                let slots: [Slot; 3] = slots.try_into().map_err(|_| windows::core::Error::from_win32())?;
                Ok(Texture { source, keyed, slots, next: 0, width: desc.Width, height: desc.Height, rgba })
            }
        }

        fn open_texture(&mut self, handle: u64) -> bool {
            if self.textures.contains_key(&handle) { return true; }
            if self.textures.len() >= 8 { self.reset(); }
            if let Some((preferred, _)) = preferred_adapter() {
                match self.open_on(preferred, handle) {
                    Ok(texture) => { self.textures.insert(handle, texture); return true; }
                    Err(_) => self.report("Prism capture: shared texture unavailable on preferred GPU; trying hardware adapters"),
                }
            }
            let adapters = adapter_list();
            for (index, _) in adapters {
                if self.adapter == Some(index) { continue; }
                if let Ok(texture) = self.open_on(index, handle) {
                    log(&format!("Prism capture: compositor texture opened on DXGI adapter {index}"));
                    self.textures.insert(handle, texture);
                    return true;
                }
            }
            self.report("Prism capture: OpenSharedResource failed on every hardware adapter");
            false
        }

        fn present(&mut self, handle: u64, frame: u64, pose: PrismFramePose) -> u64 {
            if self.last_frame == Some(frame) { return 0; }
            self.last_frame = Some(frame);
            if !self.open_texture(handle) { return 0; }
            let Some(texture) = self.textures.get_mut(&handle) else { return 0 };
            let (Some(device), Some(context)) = (&self.device, &self.context) else { return 0 };
            unsafe {
                if texture.keyed.AcquireSync(0, 0).is_err() { return 0; }
                let slot_index = texture.next;
                texture.next = (texture.next + 1) % texture.slots.len();
                let slot = &mut texture.slots[slot_index];
                context.CopyResource(&slot.stage, &texture.source);
                context.End(&slot.ready);
                if texture.keyed.ReleaseSync(0).is_err() { self.report("Prism capture: keyed mutex release failed"); return 0; }
                slot.frame = frame;
                slot.pose = pose;
                slot.pending = true;

                let mut newest: Option<usize> = None;
                for (index, slot) in texture.slots.iter().enumerate() {
                    if !slot.pending { continue; }
                    let mut done = 0i32;
                    if context.GetData(&slot.ready, Some(&mut done as *mut i32 as *mut c_void), 4, D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32).is_ok()
                        && done != 0 && newest.map_or(true, |prev| slot.frame > texture.slots[prev].frame)
                    { newest = Some(index); }
                }
                let Some(ready_index) = newest else { return 0 };
                let ready = &mut texture.slots[ready_index];
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                if context.Map(&ready.stage, 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32, Some(&mut mapped)).is_err() { return 0; }
                let ready_frame = ready.frame;
                let sequence = prism_ipc_publish_frame(mapped.pData as *const u8, mapped.RowPitch as usize,
                    texture.width, texture.height, texture.rgba, &ready.pose);
                context.Unmap(&ready.stage, 0);
                for slot in texture.slots.iter_mut() { if slot.frame <= ready_frame { slot.pending = false; } }
                let _ = device;
                sequence
            }
        }
    }

    fn capture() -> &'static Mutex<Capture> { static INSTANCE: OnceLock<Mutex<Capture>> = OnceLock::new(); INSTANCE.get_or_init(|| Mutex::new(Capture::new())) }

    #[no_mangle]
    pub extern "C" fn prism_gpu_set_logger(callback: LogFn) { if let Ok(mut log_fn) = LOG_FN.lock() { *log_fn = callback; } }

    #[no_mangle]
    pub unsafe extern "C" fn prism_gpu_preferred_luid(out: *mut u64) -> bool {
        if out.is_null() { return false; }
        let Some((_, desc)) = preferred_adapter() else { return false };
        *out = luid_u64(desc.AdapterLuid);
        true
    }

    #[no_mangle]
    pub unsafe extern "C" fn prism_gpu_present(handle: u64, frame: u64, pose: *const PrismFramePose, width: *mut u32, height: *mut u32, luid: *mut u64) -> u64 {
        if pose.is_null() || width.is_null() || height.is_null() || luid.is_null() { return 0; }
        let Ok(mut state) = capture().try_lock() else { return 0 };
        let sequence = state.present(handle, frame, *pose);
        if sequence != 0 {
            if let Some(texture) = state.textures.get(&handle) { *width = texture.width; *height = texture.height; FRAME_WIDTH.store(texture.width, Ordering::Relaxed); FRAME_HEIGHT.store(texture.height, Ordering::Relaxed); }
            if let Some(index) = state.adapter {
                if let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() {
                    if let Ok(adapter) = factory.EnumAdapters1(index) {
                        if let Ok(desc) = adapter.GetDesc1() { *luid = luid_u64(desc.AdapterLuid); }
                    }
                }
            }
        }
        sequence
    }

    #[no_mangle]
    pub extern "C" fn prism_gpu_reset() { if let Ok(mut state) = capture().lock() { state.reset(); state.last_frame = None; } }

    #[no_mangle]
    pub unsafe extern "C" fn prism_gpu_dimensions(width: *mut u32, height: *mut u32) -> bool {
        if width.is_null() || height.is_null() { return false; }
        *width = FRAME_WIDTH.load(Ordering::Relaxed); *height = FRAME_HEIGHT.load(Ordering::Relaxed); true
    }

    #[no_mangle]
    pub unsafe extern "C" fn prism_gpu_eye_viewport(eye: u32, x: *mut u32, y: *mut u32, width: *mut u32, height: *mut u32) -> bool {
        if eye > 1 || x.is_null() || y.is_null() || width.is_null() || height.is_null() { return false; }
        let frame_width = FRAME_WIDTH.load(Ordering::Relaxed); let frame_height = FRAME_HEIGHT.load(Ordering::Relaxed);
        *x = eye * (frame_width / 2); *y = 0; *width = frame_width / 2; *height = frame_height; true
    }

}

#[cfg(windows)]
pub fn reset_capture() { platform::prism_gpu_reset(); }

#[cfg(not(windows))]
#[no_mangle]
pub extern "C" fn prism_gpu_set_logger(_: Option<extern "C" fn(*const i8)>) {}
#[cfg(not(windows))]
#[no_mangle]
pub unsafe extern "C" fn prism_gpu_preferred_luid(_: *mut u64) -> bool { false }
#[cfg(not(windows))]
#[no_mangle]
pub unsafe extern "C" fn prism_gpu_present(_: u64, _: u64, _: *const PrismFramePose, _: *mut u32, _: *mut u32, _: *mut u64) -> u64 { 0 }
#[cfg(not(windows))]
#[no_mangle]
pub extern "C" fn prism_gpu_reset() {}

#[cfg(not(windows))]
#[no_mangle]
pub unsafe extern "C" fn prism_gpu_dimensions(width: *mut u32, height: *mut u32) -> bool {
    if width.is_null() || height.is_null() { return false; }
    *width=2048;*height=1024;true
}

#[cfg(not(windows))]
#[no_mangle]
pub unsafe extern "C" fn prism_gpu_eye_viewport(eye: u32, x: *mut u32, y: *mut u32, width: *mut u32, height: *mut u32) -> bool {
    if eye > 1 || x.is_null() || y.is_null() || width.is_null() || height.is_null() { return false; }
    *x=eye*1024;*y=0;*width=1024;*height=1024;true
}

#[cfg(not(windows))]
pub(super) fn reset_capture() { prism_gpu_reset(); }
