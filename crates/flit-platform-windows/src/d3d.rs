use crate::err;
use crate::window::{FrameSync, Win32Window};
use flit_core::atlas::Slot;
use flit_core::render::{DrawList, Instance};
use flit_core::thumbs::THUMB_ATLAS;
use flit_core::ui::Bitmap;
use flit_platform::{PlatformError, RenderError, Renderer, Window};
use std::cell::Cell;
use std::rc::Rc;
use std::thread::ThreadId;
use windows::core::{Interface, HRESULT, PCSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::WaitForSingleObjectEx;

static VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui_vs.cso"));
static PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui_ps.cso"));

#[repr(C)]
struct Globals {
    viewport: [f32; 2],
    text_contrast: f32,
    _pad: f32,
}

#[derive(Clone)]
struct Gpu {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    raster: ID3D11RasterizerState,
}

pub struct D3dDevice {
    gpu: Gpu,
    render_thread: Cell<Option<ThreadId>>,
    pub warp: bool,
}

unsafe impl Send for D3dDevice {}

struct Waitable(HANDLE);

impl Drop for Waitable {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub struct D3dRenderer {
    dev: Gpu,
    waitable: Waitable,
    swap: IDXGISwapChain2,
    _dcomp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    rtv: Option<ID3D11RenderTargetView>,
    instances: ID3D11Buffer,
    capacity: usize,
    globals: ID3D11Buffer,
    atlas: ID3D11Texture2D,
    atlas_srv: ID3D11ShaderResourceView,
    thumbs: Option<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
    preview: Option<ID3D11ShaderResourceView>,
    size: (u32, u32),
    sync: Rc<FrameSync>,
}

fn texture(device: &ID3D11Device, size: u32) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView), PlatformError> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size,
        Height: size,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        ..Default::default()
    };
    let (mut tex, mut srv) = (None, None);
    unsafe {
        device.CreateTexture2D(&desc, None, Some(&mut tex)).map_err(err)?;
        let tex = tex.unwrap();
        device.CreateShaderResourceView(&tex, None, Some(&mut srv)).map_err(err)?;
        Ok((tex, srv.unwrap()))
    }
}

fn update(ctx: &ID3D11DeviceContext, tex: &ID3D11Texture2D, slot: Slot, rgba: &[u8]) {
    let b = D3D11_BOX { left: slot.x, top: slot.y, front: 0, right: slot.x + slot.w, bottom: slot.y + slot.h, back: 1 };
    unsafe {
        ctx.UpdateSubresource(tex, 0, Some(&b), rgba.as_ptr().cast(), slot.w * 4, 0);
    }
}

const SWAP_FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

fn create_device(driver: D3D_DRIVER_TYPE) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let (mut device, mut ctx) = (None, None);
    let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0];
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )?;
    }
    Ok((device.unwrap(), ctx.unwrap()))
}

fn buffer(device: &ID3D11Device, bytes: usize, bind: D3D11_BIND_FLAG) -> Result<ID3D11Buffer, PlatformError> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: bytes as u32,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        ..Default::default()
    };
    let mut b = None;
    unsafe { device.CreateBuffer(&desc, None, Some(&mut b)).map_err(err)? };
    Ok(b.unwrap())
}

impl D3dDevice {
    fn new(warp: bool) -> Result<Self, PlatformError> {
        let _span = tracing::info_span!("d3d_device", warp).entered();
        let (device, ctx) = create_device(if warp { D3D_DRIVER_TYPE_WARP } else { D3D_DRIVER_TYPE_HARDWARE }).map_err(err)?;
        unsafe {
            let (mut vs, mut ps, mut layout) = (None, None, None);
            device.CreateVertexShader(VS, None, Some(&mut vs)).map_err(err)?;
            device.CreatePixelShader(PS, None, Some(&mut ps)).map_err(err)?;
            let el = |name: &'static [u8], format: DXGI_FORMAT, offset: u32| D3D11_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(name.as_ptr()),
                SemanticIndex: 0,
                Format: format,
                InputSlot: 0,
                AlignedByteOffset: offset,
                InputSlotClass: D3D11_INPUT_PER_INSTANCE_DATA,
                InstanceDataStepRate: 1,
            };
            let elements = [
                el(b"RECT\0", DXGI_FORMAT_R32G32B32A32_FLOAT, 0),
                el(b"UV\0", DXGI_FORMAT_R32G32B32A32_FLOAT, 16),
                el(b"CLIP\0", DXGI_FORMAT_R32G32B32A32_FLOAT, 32),
                el(b"COLOR\0", DXGI_FORMAT_R8G8B8A8_UNORM, 48),
                el(b"PARAMS\0", DXGI_FORMAT_R32G32_FLOAT, 52),
                el(b"KIND\0", DXGI_FORMAT_R32_UINT, 60),
            ];
            device.CreateInputLayout(&elements, VS, Some(&mut layout)).map_err(err)?;
            let mut sampler = None;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            device.CreateSamplerState(&sd, Some(&mut sampler)).map_err(err)?;
            let mut bd = D3D11_BLEND_DESC::default();
            bd.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut blend = None;
            device.CreateBlendState(&bd, Some(&mut blend)).map_err(err)?;
            let rd = D3D11_RASTERIZER_DESC { FillMode: D3D11_FILL_SOLID, CullMode: D3D11_CULL_NONE, DepthClipEnable: true.into(), ..Default::default() };
            let mut raster = None;
            device.CreateRasterizerState(&rd, Some(&mut raster)).map_err(err)?;
            let gpu = Gpu {
                device,
                ctx,
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                layout: layout.unwrap(),
                sampler: sampler.unwrap(),
                blend: blend.unwrap(),
                raster: raster.unwrap(),
            };
            Ok(Self { gpu, render_thread: Cell::new(None), warp })
        }
    }

    fn bind_to_current_thread(&self) -> Result<(), PlatformError> {
        let me = std::thread::current().id();
        let owner = *self.render_thread.get().get_or_insert(me);
        self.render_thread.set(Some(owner));
        if owner != me {
            return Err(PlatformError::new(-1, "the graphics device already renders on another thread"));
        }
        Ok(())
    }
}

fn device_lost(code: HRESULT) -> bool {
    code == DXGI_ERROR_DEVICE_REMOVED || code == DXGI_ERROR_DEVICE_RESET || code == DXGI_ERROR_DEVICE_HUNG
}

impl D3dRenderer {
    fn write<T>(&self, buf: &ID3D11Buffer, data: &[T]) -> Result<(), PlatformError> {
        unsafe {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.dev.ctx.Map(buf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).map_err(err)?;
            std::ptr::copy_nonoverlapping(data.as_ptr() as *const u8, m.pData as *mut u8, std::mem::size_of_val(data));
            self.dev.ctx.Unmap(buf, 0);
        }
        Ok(())
    }

    fn classify(&self, e: PlatformError) -> RenderError {
        if device_lost(HRESULT(e.code as i32)) || unsafe { self.dev.device.GetDeviceRemovedReason() }.is_err() {
            RenderError::DeviceLost
        } else {
            RenderError::Other(e)
        }
    }

    fn draw(&mut self, list: &DrawList) -> Result<(), PlatformError> {
        unsafe {
            let waitable = self.sync.waitable.get();
            if !self.sync.waited.replace(false) && !waitable.is_invalid() {
                WaitForSingleObjectEx(waitable, 100, true);
            }
            if self.rtv.is_none() {
                let back: ID3D11Texture2D = self.swap.GetBuffer(0).map_err(err)?;
                let mut rtv = None;
                self.dev.device.CreateRenderTargetView(&back, None, Some(&mut rtv)).map_err(err)?;
                self.rtv = rtv;
            }
            let n = list.instances.len();
            if n > self.capacity {
                self.capacity = n.next_power_of_two();
                self.instances = buffer(&self.dev.device, self.capacity * size_of::<Instance>(), D3D11_BIND_VERTEX_BUFFER)?;
            }
            if n > 0 {
                self.write(&self.instances, &list.instances)?;
            }
            let (w, h) = self.size;
            self.write(&self.globals, &[Globals { viewport: [w as f32, h as f32], text_contrast: 0.5, _pad: 0.0 }])?;

            let d = &self.dev;
            let ctx = &d.ctx;
            ctx.OMSetRenderTargets(Some(std::slice::from_ref(&self.rtv)), None);
            ctx.ClearRenderTargetView(self.rtv.as_ref().unwrap(), &list.clear.to_f32());
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { Width: w as f32, Height: h as f32, MaxDepth: 1.0, ..Default::default() }]));
            ctx.RSSetState(&d.raster);
            ctx.OMSetBlendState(&d.blend, None, u32::MAX);
            ctx.IASetInputLayout(&d.layout);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            let stride = size_of::<Instance>() as u32;
            ctx.IASetVertexBuffers(0, 1, Some(&Some(self.instances.clone())), Some(&stride), Some(&0));
            ctx.VSSetShader(&d.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.globals.clone())]));
            ctx.PSSetShader(&d.ps, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.globals.clone())]));
            ctx.PSSetShaderResources(0, Some(&[Some(self.atlas_srv.clone()), self.thumbs.as_ref().map(|t| t.1.clone()), self.preview.clone()]));
            ctx.PSSetSamplers(0, Some(&[Some(d.sampler.clone())]));
            if n > 0 {
                ctx.DrawInstanced(4, n as u32, 0, 0);
            }
            if let Some(path) = capture_path() {
                self.capture(path)?;
            }
            self.swap.Present(1, DXGI_PRESENT(0)).ok().map_err(err)
        }
    }

    unsafe fn capture(&self, path: &std::path::Path) -> Result<(), PlatformError> {
        static EXIT: std::sync::Once = std::sync::Once::new();
        EXIT.call_once(|| {
            let ms = std::env::var("FLIT_CAPTURE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(4000);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                std::process::exit(0);
            });
        });
        let back: ID3D11Texture2D = self.swap.GetBuffer(0).map_err(err)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        back.GetDesc(&mut desc);
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        self.dev.device.CreateTexture2D(&desc, None, Some(&mut staging)).map_err(err)?;
        let Some(staging) = staging else { return Ok(()) };
        self.dev.ctx.CopyResource(&staging, &back);
        let mut map = D3D11_MAPPED_SUBRESOURCE::default();
        self.dev.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map)).map_err(err)?;
        let (w, h) = (desc.Width as usize, desc.Height as usize);
        let mut raw = Vec::with_capacity((w * 3 + 1) * h);
        for y in 0..h {
            let row = std::slice::from_raw_parts((map.pData as *const u8).add(y * map.RowPitch as usize), w * 4);
            raw.push(0);
            raw.extend(row.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]));
        }
        self.dev.ctx.Unmap(&staging, 0);
        let tmp = path.with_extension("tmp");
        let _ = std::fs::write(&tmp, png(w as u32, h as u32, &raw)).and_then(|_| std::fs::rename(&tmp, path));
        Ok(())
    }
}

pub fn capture_path() -> Option<&'static std::path::Path> {
    static PATH: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| std::env::var_os("FLIT_HIDDEN_CAPTURE").map(Into::into)).as_deref()
}

fn png(w: u32, h: u32, raw: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(0xFFFF).collect();
    for (i, b) in blocks.iter().enumerate() {
        let n = b.len() as u16;
        z.push(u8::from(i + 1 == blocks.len()));
        z.extend(n.to_le_bytes());
        z.extend((!n).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (a, b) = raw.iter().fold((1u32, 0u32), |(a, b), &x| ((a + x as u32) % 65521, (b + a + x as u32) % 65521));
    z.extend(((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend((data.len() as u32).to_be_bytes());
        let body = [kind, data].concat();
        out.extend_from_slice(&body);
        out.extend(crc32fast::hash(&body).to_be_bytes());
    };
    chunk(b"IHDR", &[&w.to_be_bytes()[..], &h.to_be_bytes(), &[8, 2, 0, 0, 0]].concat());
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

impl Drop for D3dRenderer {
    fn drop(&mut self) {
        if self.sync.waitable.get() == self.waitable.0 {
            self.sync.waitable.set(HANDLE::default());
            self.sync.waited.set(false);
        }
    }
}

impl Renderer for D3dRenderer {
    type Window = Win32Window;
    type Device = D3dDevice;

    fn device(software: bool) -> Result<D3dDevice, PlatformError> {
        D3dDevice::new(software)
    }

    fn new(dev: &D3dDevice, window: &Win32Window, atlas_size: u32) -> Result<Self, PlatformError> {
        let _span = tracing::info_span!("renderer_new", warp = dev.warp).entered();
        dev.bind_to_current_thread()?;
        let device = &dev.gpu.device;
        unsafe {
            let dxgi: IDXGIDevice = device.cast().map_err(err)?;
            let factory: IDXGIFactory2 = dxgi.GetAdapter().map_err(err)?.GetParent().map_err(err)?;
            let (w, h) = window.size();
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w.max(1),
                Height: h.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: SWAP_FLAGS.0 as u32,
                ..Default::default()
            };
            let swap: IDXGISwapChain2 = factory.CreateSwapChainForComposition(device, &desc, None).map_err(err)?.cast().map_err(err)?;
            swap.SetMaximumFrameLatency(1).map_err(err)?;
            let waitable = Waitable(swap.GetFrameLatencyWaitableObject());

            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi).map_err(err)?;
            let target = dcomp.CreateTargetForHwnd(window.hwnd(), false).map_err(err)?;
            let visual = dcomp.CreateVisual().map_err(err)?;
            visual.SetContent(&swap).map_err(err)?;
            target.SetRoot(&visual).map_err(err)?;
            dcomp.Commit().map_err(err)?;

            let capacity = 16 * 1024;
            let instances = buffer(device, capacity * size_of::<Instance>(), D3D11_BIND_VERTEX_BUFFER)?;
            let globals = buffer(device, size_of::<Globals>(), D3D11_BIND_CONSTANT_BUFFER)?;
            let (atlas, atlas_srv) = texture(device, atlas_size)?;

            window.sync.waitable.set(waitable.0);
            window.sync.waited.set(false);
            tracing::info!(warp = dev.warp, "renderer ready");
            Ok(Self {
                dev: dev.gpu.clone(),
                waitable,
                swap,
                _dcomp: dcomp,
                _target: target,
                _visual: visual,
                rtv: None,
                instances,
                capacity,
                globals,
                atlas,
                atlas_srv,
                thumbs: None,
                preview: None,
                size: (w.max(1), h.max(1)),
                sync: window.sync.clone(),
            })
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == self.size {
            return;
        }
        self.size = (width, height);
        self.rtv = None;
        unsafe {
            self.dev.ctx.OMSetRenderTargets(None, None);
            self.dev.ctx.Flush();
            if let Err(e) = self.swap.ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, SWAP_FLAGS) {
                tracing::warn!("ResizeBuffers: {e}");
            }
        }
    }

    fn upload(&mut self, slot: Slot, rgba: &[u8]) {
        update(&self.dev.ctx, &self.atlas, slot, rgba);
    }

    fn upload_thumb(&mut self, slot: Slot, rgba: &[u8]) {
        if self.thumbs.is_none() {
            match texture(&self.dev.device, THUMB_ATLAS) {
                Ok(t) => self.thumbs = Some(t),
                Err(e) => return tracing::warn!("thumbnail texture: {e}"),
            }
        }
        if let Some((tex, _)) = &self.thumbs {
            update(&self.dev.ctx, tex, slot, rgba);
        }
    }

    fn upload_preview(&mut self, image: Option<&Bitmap>) {
        self.preview = image.and_then(|b| {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: b.width,
                Height: b.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_IMMUTABLE,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };
            let data = D3D11_SUBRESOURCE_DATA { pSysMem: b.rgba.as_ptr().cast(), SysMemPitch: b.width * 4, SysMemSlicePitch: 0 };
            let (mut tex, mut srv) = (None, None);
            unsafe {
                self.dev.device.CreateTexture2D(&desc, Some(&data), Some(&mut tex)).map_err(|e| tracing::warn!("preview texture: {e}")).ok()?;
                self.dev.device.CreateShaderResourceView(tex.as_ref()?, None, Some(&mut srv)).ok()?;
            }
            srv
        });
    }

    fn render(&mut self, list: &DrawList) -> Result<(), RenderError> {
        let _span = tracing::trace_span!("render", n = list.instances.len()).entered();
        self.draw(list).map_err(|e| self.classify(e))
    }
}
