//! Own-device asynchronous GPU readback. All D3D operations belong to one worker;
//! the SteamVR Present callback only replaces a raw presentation descriptor.
use crate::{
    ipc::Mapping,
    latest::{Latest, Poll},
    protocol::Tracking,
};
use std::{collections::HashMap, sync::Arc, thread};
use windows::{
    core::Interface,
    Win32::{
        Foundation::HANDLE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
};
#[derive(Clone, Copy)]
pub struct Presentation {
    pub handle: u64,
    pub frame: u64,
    pub tracking: Tracking,
}
struct Slot {
    texture: ID3D11Texture2D,
    query: ID3D11Query,
    pending: bool,
    frame: u64,
    tracking: Tracking,
}
struct Source {
    texture: ID3D11Texture2D,
    keyed: IDXGIKeyedMutex,
    width: u32,
    height: u32,
    rgba: bool,
    slots: Vec<Slot>,
    next: usize,
}
struct Device {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    sources: HashMap<u64, Source>,
}
impl Device {
    unsafe fn open_on(index: u32, handle: u64) -> windows::core::Result<Self> {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let adapter: IDXGIAdapter = factory.EnumAdapters1(index)?.cast()?;
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            Some(&adapter),
            D3D_DRIVER_TYPE_UNKNOWN,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let mut result = Self {
            device: device.unwrap(),
            context: context.unwrap(),
            sources: HashMap::new(),
        };
        result.open(handle)?;
        Ok(result)
    }
    unsafe fn open(&mut self, handle: u64) -> windows::core::Result<()> {
        if self.sources.contains_key(&handle) {
            return Ok(());
        }
        let mut source: Option<ID3D11Texture2D> = None;
        self.device
            .OpenSharedResource(HANDLE(handle as *mut core::ffi::c_void), &mut source)?;
        let texture = source.ok_or_else(windows::core::Error::from_win32)?;
        let keyed: IDXGIKeyedMutex = texture.cast()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let rgba = desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM
            || desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
        let bgra = desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM
            || desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM_SRGB;
        if (!rgba && !bgra)
            || desc.Width < 512
            || desc.Width > 4096
            || desc.Height < 256
            || desc.Height > 2048
            || desc.SampleDesc.Count != 1
            || desc.ArraySize != 1
            || desc.MipLevels != 1
        {
            return Err(windows::core::Error::from_win32());
        }
        let width = desc.Width;
        let height = desc.Height;
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.MiscFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut slots = Vec::new();
        for _ in 0..3 {
            let mut stage = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut stage))?;
            let mut query = None;
            self.device.CreateQuery(
                &D3D11_QUERY_DESC {
                    Query: D3D11_QUERY_EVENT,
                    MiscFlags: 0,
                },
                Some(&mut query),
            )?;
            slots.push(Slot {
                texture: stage.unwrap(),
                query: query.unwrap(),
                pending: false,
                frame: 0,
                tracking: Tracking::default(),
            });
        }
        // Shared handles may be recycled after a game/mode transition. Keep a
        // bounded cache; closing old COM resources never changes image geometry.
        if self.sources.len() >= 4 {
            self.sources.clear();
        }
        self.sources.insert(
            handle,
            Source {
                texture,
                keyed,
                width,
                height,
                rgba,
                slots,
                next: 0,
            },
        );
        Ok(())
    }
    unsafe fn submit(&mut self, p: Presentation, map: &Mapping) -> windows::core::Result<()> {
        self.open(p.handle)?;
        let source = self.sources.get_mut(&p.handle).unwrap();
        Self::drain_source(&self.context, source, map)?;
        let Some(i) = (0..source.slots.len())
            .map(|offset| (source.next + offset) % source.slots.len())
            .find(|i| !source.slots[*i].pending)
        else {
            return Ok(());
        };
        if source.keyed.AcquireSync(0, 0).is_err() {
            return Ok(());
        }
        let slot = &mut source.slots[i];
        self.context.CopyResource(&slot.texture, &source.texture);
        self.context.End(&slot.query);
        // A dedicated context has no Present call to submit queued commands.
        // Flush is required here; waiting for implicit driver batching adds lag.
        self.context.Flush();
        source.keyed.ReleaseSync(0)?;
        slot.pending = true;
        slot.frame = p.frame;
        slot.tracking = p.tracking;
        source.next = (i + 1) % source.slots.len();
        Ok(())
    }
    unsafe fn drain_source(
        context: &ID3D11DeviceContext,
        source: &mut Source,
        map: &Mapping,
    ) -> windows::core::Result<()> {
        let mut newest = None;
        // Consume completed slots before reusing a stage; metadata always stays
        // attached to the GPU copy that produced that exact pixel buffer.
        for (i, slot) in source.slots.iter().enumerate() {
            if !slot.pending {
                continue;
            }
            let mut done = 0i32;
            context.GetData(
                &slot.query,
                Some((&mut done as *mut i32).cast()),
                4,
                D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
            )?;
            if done != 0 && newest.map_or(true, |old: usize| slot.frame > source.slots[old].frame) {
                newest = Some(i);
            }
        }
        if let Some(i) = newest {
            let slot = &source.slots[i];
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if context
                .Map(
                    &slot.texture,
                    0,
                    D3D11_MAP_READ,
                    D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                    Some(&mut mapped),
                )
                .is_ok()
            {
                map.publish(
                    mapped.pData.cast(),
                    mapped.RowPitch as usize,
                    source.width,
                    source.height,
                    source.rgba,
                    slot.tracking,
                );
                context.Unmap(&slot.texture, 0);
                let frame = slot.frame;
                for old in &mut source.slots {
                    if old.frame <= frame {
                        old.pending = false;
                    }
                }
            }
        }
        Ok(())
    }
    unsafe fn drain(&mut self, map: &Mapping) -> windows::core::Result<()> {
        for source in self.sources.values_mut() {
            Self::drain_source(&self.context, source, map)?;
        }
        Ok(())
    }
}
pub struct Capture {
    queue: Arc<Latest<Presentation>>,
    join: Option<thread::JoinHandle<()>>,
}
impl Capture {
    pub fn new(map: Arc<Mapping>) -> Option<Self> {
        let queue = Arc::new(Latest::<Presentation>::new());
        let input = queue.clone();
        let join=thread::Builder::new().name("aperture-gpu-capture".into()).spawn(move||{
            let mut device:Option<Device>=None;let mut last=None;
            loop {
                let p=match input.receive_timeout(std::time::Duration::from_millis(1)) {
                    Poll::Closed=>break,
                    Poll::Timeout=>{
                        let failure=device.as_mut().and_then(|active|unsafe{active.drain(&map).err()});
                        if let Some(e)=failure{crate::log(&format!("Aperture: GPU drain error {e}"));device=None;}
                        continue;
                    },
                    Poll::Value(p)=>p,
                };
                if last==Some((p.handle,p.frame)){continue;}last=Some((p.handle,p.frame));
                unsafe {
                    if device.is_none() {
                        for index in 0..16 {if let Ok(opened)=Device::open_on(index,p.handle){device=Some(opened);break;}}
                        if device.is_none(){crate::log("Aperture: could not open compositor texture on any DXGI adapter");continue;}
                    }
                    if let Err(error)=device.as_mut().unwrap().submit(p,&map) {
                        crate::log(&format!("Aperture: GPU capture error {error}"));device=None;
                    }
                }
            }
        }).ok()?;
        Some(Self {
            queue,
            join: Some(join),
        })
    }
    pub fn present(&self, p: Presentation) {
        self.queue.publish(p);
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.queue.close();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore]
    fn isolated_shared_gpu_frame_exports_without_a_second_present() {
        // This creates actual D3D11 shared resources and a private IPC mapping.
        // It never initializes SteamVR, claims HMD tracking, or opens live IPC.
        unsafe {
            let fixture = crate::ipc::Fixture::new();
            let factory: IDXGIFactory1 = CreateDXGIFactory1().unwrap();
            let adapter: IDXGIAdapter = factory.EnumAdapters1(0).unwrap().cast().unwrap();
            let (mut device, mut context) = (None, None);
            D3D11CreateDevice(
                Some(&adapter),
                D3D_DRIVER_TYPE_UNKNOWN,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .unwrap();
            let device = device.unwrap();
            let context = context.unwrap();
            let (width, height) = (1024u32, 256u32);
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
            };
            let mut texture = None;
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .unwrap();
            let texture = texture.unwrap();
            let keyed: IDXGIKeyedMutex = texture.cast().unwrap();
            keyed.AcquireSync(0, 1000).unwrap();
            let pixels: Vec<u8> = (0..height)
                .flat_map(|_| {
                    (0..width).flat_map(|x| {
                        if x < width / 2 {
                            [0, 0, 255, 255]
                        } else {
                            [255, 0, 0, 255]
                        }
                    })
                })
                .collect();
            context.UpdateSubresource(&texture, 0, None, pixels.as_ptr().cast(), width * 4, 0);
            context.Flush();
            keyed.ReleaseSync(0).unwrap();
            let resource: IDXGIResource = texture.cast().unwrap();
            let handle = resource.GetSharedHandle().unwrap();
            let capture = Capture::new(fixture.mapping.clone()).unwrap();
            capture.present(Presentation {
                handle: handle.0 as u64,
                frame: 17,
                tracking: Tracking {
                    sample_us: 123,
                    ..Tracking::default()
                },
            });
            let start = std::time::Instant::now();
            while fixture
                .mapping
                .snapshot()
                .map_or(true, |s| s.header.frame_sequence == 0)
            {
                assert!(
                    start.elapsed() < std::time::Duration::from_secs(3),
                    "Single shared GPU frame was not exported before deadline"
                );
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert_eq!(fixture.mapping.snapshot().unwrap().header.frame_sequence, 1);
            assert_eq!(fixture.pixels(width, height), pixels);
            println!("GPU fixture PASS: exact red/blue BGRA frame exported after one Present in {:.2} ms; no live HMD fixture",start.elapsed().as_secs_f64()*1000.);
            drop(capture);
        }
    }
}
