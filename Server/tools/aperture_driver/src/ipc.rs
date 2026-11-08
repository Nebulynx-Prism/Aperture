use crate::protocol::*;
use core::ffi::c_void;
type Handle = *mut c_void;
#[link(name = "kernel32")]
extern "system" {
    fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn OpenMutexW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn MapViewOfFile(
        mapping: Handle,
        access: u32,
        high: u32,
        low: u32,
        bytes: usize,
    ) -> *mut c_void;
    fn UnmapViewOfFile(view: *const c_void) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn ReleaseMutex(handle: Handle) -> i32;
    fn GetTickCount64() -> u64;
}
pub fn now_ms() -> u64 {
    unsafe { GetTickCount64() }
}
pub struct Mapping {
    mapping: Handle,
    mutex: Handle,
    view: *mut u8,
}
// Every shared-memory read/write holds the named cross-process mutex.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}
struct Unlock(Handle);
impl Drop for Unlock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
        }
    }
}
impl Mapping {
    pub fn open() -> Option<Self> {
        Self::open_named(
            "Local\\PrismAperture.OpenVR.v1",
            "Local\\PrismAperture.OpenVR.Mutex.v1",
        )
    }
    fn open_named(name: &str, mutex_name: &str) -> Option<Self> {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mutex_name: Vec<u16> = mutex_name.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let mapping = OpenFileMappingW(0x000f001f, 0, name.as_ptr());
            if mapping.is_null() {
                return None;
            }
            let mutex = OpenMutexW(0x00100001, 0, mutex_name.as_ptr());
            if mutex.is_null() {
                CloseHandle(mapping);
                return None;
            }
            let view = MapViewOfFile(mapping, 0x000f001f, 0, 0, MAPPING_BYTES).cast::<u8>();
            if view.is_null() {
                CloseHandle(mutex);
                CloseHandle(mapping);
                return None;
            }
            Some(Self {
                mapping,
                mutex,
                view,
            })
        }
    }
    fn locked<T>(&self, f: impl FnOnce(*mut u8) -> T) -> Option<T> {
        let result = unsafe { WaitForSingleObject(self.mutex, 0) };
        if result != 0 && result != 0x80 {
            return None;
        }
        let _unlock = Unlock(self.mutex);
        Some(f(self.view))
    }
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.locked(|view| unsafe {
            let header = std::ptr::read_unaligned(view.cast::<Header>());
            if header.magic != MAGIC || header.version != 1 {
                return None;
            }
            let identity = std::ptr::read_unaligned(view.add(264).cast::<[u32; 4]>());
            let valid = identity[0] == 0x49445650 && identity[1] == 1;
            let motion = std::ptr::read_unaligned(view.add(MOTION_OFFSET).cast::<Motion>());
            let motion = if motion.valid() && motion.sample_us == header.tracking.sample_us {
                Some(motion)
            } else {
                None
            };
            let alignment =
                std::ptr::read_unaligned(view.add(ALIGNED_OFFSET).cast::<AlignedMotion>());
            let aligned = if alignment.valid_for(
                header.tracking.sample_us,
                aperture_wire::clock::host_now_ns(),
            ) {
                Some(alignment)
            } else {
                None
            };
            Some(Snapshot {
                header,
                headset: if valid { identity[2] } else { 0 },
                eye_width: if valid { identity[3] & 0xffff } else { 0 },
                eye_height: if valid { identity[3] >> 16 } else { 0 },
                motion,
                aligned,
            })
        })?
    }
    pub fn beat(&self, ready: bool) {
        self.locked(|view| unsafe {
            let h = &mut *view.cast::<Header>();
            if h.magic == MAGIC && h.version == 1 {
                h.driver_ms = now_ms();
                h.ready = ready as u32;
            }
        });
    }
    pub fn configure_output(&self, width: u32, height: u32) {
        self.locked(|view| unsafe {
            let h = &mut *view.cast::<Header>();
            if h.magic == MAGIC && h.version == 1 && h.frame_sequence == 0 {
                h.width = width;
                h.height = height;
                h.stride = width * 4;
                h.format = 0;
            }
            // sequence/time are intentionally untouched: configuration is not a frame.
        });
    }
    /// The pixel copy and sequence publish are atomic to the existing receiver.
    /// Contention drops a raw frame instead of blocking the compositor.
    pub unsafe fn publish(
        &self,
        src: *const u8,
        pitch: usize,
        width: u32,
        height: u32,
        rgba: bool,
        pose: Tracking,
    ) -> bool {
        if src.is_null()
            || !(512..=MAX_WIDTH).contains(&width)
            || !(256..=MAX_HEIGHT).contains(&height)
            || width % 2 != 0
            || pitch < (width as usize * 4)
        {
            return false;
        }
        self.locked(|view| {
            let h = &mut *view.cast::<Header>();
            if h.magic != MAGIC || h.version != 1 {
                return false;
            }
            for y in 0..height as usize {
                let input = src.add(y * pitch);
                let output = view.add(PIXELS + y * width as usize * 4);
                if rgba {
                    for x in 0..width as usize {
                        let p = input.add(x * 4);
                        let q = output.add(x * 4);
                        *q = *p.add(2);
                        *q.add(1) = *p.add(1);
                        *q.add(2) = *p;
                        *q.add(3) = *p.add(3);
                    }
                } else {
                    std::ptr::copy_nonoverlapping(input, output, width as usize * 4);
                }
            }
            let sequence = h.frame_sequence.wrapping_add(1);
            let metadata = FramePose {
                magic: 0x46505650,
                version: 1,
                sequence,
                head: pose.head,
                fov: pose.fov,
                sample_us: pose.sample_us,
            };
            std::ptr::write_unaligned(view.add(280).cast::<FramePose>(), metadata);
            h.width = width;
            h.height = height;
            h.stride = width * 4;
            h.format = 0;
            h.frame_ms = now_ms();
            h.frame_sequence = sequence;
            h.driver_ms = now_ms();
            h.ready = 1;
            true
        })
        .unwrap_or(false)
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view.cast());
            CloseHandle(self.mutex);
            CloseHandle(self.mapping);
        }
    }
}

#[cfg(test)]
pub struct Fixture {
    pub mapping: std::sync::Arc<Mapping>,
    owner: Handle,
    owner_mutex: Handle,
}
#[cfg(test)]
impl Fixture {
    pub fn new() -> Self {
        #[link(name = "kernel32")]
        extern "system" {
            fn CreateFileMappingW(
                file: Handle,
                security: *const c_void,
                protect: u32,
                high: u32,
                low: u32,
                name: *const u16,
            ) -> Handle;
            fn CreateMutexW(security: *const c_void, owner: i32, name: *const u16) -> Handle;
        }
        let suffix = format!("{}.{}", std::process::id(), now_ms());
        let name = format!("Local\\Aperture.IsolatedGpuTest.{suffix}");
        let mutex = format!("{name}.Mutex");
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let wide_mutex: Vec<u16> = mutex.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let owner = CreateFileMappingW(
                -1isize as Handle,
                std::ptr::null(),
                4,
                0,
                MAPPING_BYTES as u32,
                wide.as_ptr(),
            );
            assert!(!owner.is_null());
            let owner_mutex = CreateMutexW(std::ptr::null(), 0, wide_mutex.as_ptr());
            assert!(!owner_mutex.is_null());
            let mapping = std::sync::Arc::new(Mapping::open_named(&name, &mutex).unwrap());
            mapping
                .locked(|p| {
                    std::ptr::write(
                        p.cast::<Header>(),
                        Header {
                            magic: MAGIC,
                            version: 1,
                            ..Header::default()
                        },
                    )
                })
                .unwrap();
            Self {
                mapping,
                owner,
                owner_mutex,
            }
        }
    }
    pub fn pixels(&self, width: u32, height: u32) -> Vec<u8> {
        self.mapping
            .locked(|p| unsafe {
                std::slice::from_raw_parts(p.add(PIXELS), width as usize * height as usize * 4)
                    .to_vec()
            })
            .unwrap()
    }
}
#[cfg(test)]
impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.owner_mutex);
            CloseHandle(self.owner);
        }
    }
}
