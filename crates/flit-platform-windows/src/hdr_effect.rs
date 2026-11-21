use crate::hdr_gpu::ToneMapper;
use std::mem::ManuallyDrop;
use std::sync::{Mutex, MutexGuard};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Media::MediaFoundation::*;
use windows_core::{implement, Error, IUnknown, Interface, Ref, Result, GUID};

#[derive(Clone, Copy)]
pub(crate) struct HdrColor {
    hlg: bool,
    peak_nits: f32,
    full_range: bool,
}

impl HdrColor {
    pub(crate) unsafe fn from_type(kind: &IMFMediaType, source: Option<Self>) -> Option<Self> {
        let primaries = kind.GetUINT32(&MF_MT_VIDEO_PRIMARIES).unwrap_or_default();
        if primaries != MFVideoPrimaries_BT2020.0 as u32 && (primaries != 0 || source.is_none()) {
            return None;
        }
        let matrix = kind.GetUINT32(&MF_MT_YUV_MATRIX).unwrap_or_default();
        if ![0, MFVideoTransferMatrix_BT2020_10.0 as u32, MFVideoTransferMatrix_BT2020_12.0 as u32].contains(&matrix) {
            return None;
        }
        let hlg = match kind.GetUINT32(&MF_MT_TRANSFER_FUNCTION).unwrap_or_default() {
            v if v == MFVideoTransFunc_2084.0 as u32 => false,
            v if v == MFVideoTransFunc_HLG.0 as u32 => true,
            0 => source?.hlg,
            _ => return None,
        };
        let peak_nits = [MF_MT_MAX_LUMINANCE_LEVEL, MF_MT_MAX_MASTERING_LUMINANCE]
            .into_iter()
            .filter_map(|key| kind.GetUINT32(&key).ok())
            .find(|value| (100..=10000).contains(value))
            .map_or_else(|| source.map_or(1000.0, |color| color.peak_nits), |n| n as f32);
        let full_range = match kind.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE).unwrap_or_default() {
            0 => source.is_some_and(|color| color.full_range),
            value => value == MFNominalRange_0_255.0 as u32,
        };
        Some(Self { hlg, peak_nits, full_range })
    }
}

pub(crate) fn create(device: &ID3D11Device, color: HdrColor) -> Result<IMFTransform> {
    ToneMapper::check_support(device)?;
    unsafe {
        let attributes = attributes()?;
        attributes.SetUINT32(&MF_SA_D3D11_AWARE, 1)?;
        Ok(Effect { attributes, state: Mutex::new(State::default()), device: device.clone(), color }.into())
    }
}

#[implement(IMFTransform)]
struct Effect {
    color: HdrColor,
    attributes: IMFAttributes,
    state: Mutex<State>,
    device: ID3D11Device,
}

#[derive(Default)]
struct State {
    input: Option<Input>,
    output: Option<IMFMediaType>,
    pending: Option<IMFSample>,
    managed_device: Option<ID3D11Device>,
    gpu: Option<Gpu>,
}

struct Input {
    kind: IMFMediaType,
    color: HdrColor,
}

struct Gpu {
    device: ID3D11Device,
    mapper: ToneMapper,
    allocator: IMFVideoSampleAllocatorEx,
}

unsafe fn attributes() -> Result<IMFAttributes> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 2)?;
    attrs.ok_or_else(Error::empty)
}

unsafe fn texture(dxgi: &IMFDXGIBuffer) -> Result<(ID3D11Texture2D, u32)> {
    let mut raw = std::ptr::null_mut();
    dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw)?;
    let surface = ID3D11Texture2D::from_raw(raw);
    Ok((surface, dxgi.GetSubresourceIndex()?))
}

unsafe fn dimensions(kind: &IMFMediaType) -> Result<(u32, u32)> {
    let size = kind.GetUINT64(&MF_MT_FRAME_SIZE)?;
    let (width, height) = ((size >> 32) as u32, size as u32);
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(MF_E_INVALIDMEDIATYPE.into());
    }
    Ok((width, height))
}

unsafe fn output_type(input: &IMFMediaType, subtype: &GUID) -> Result<IMFMediaType> {
    let output = MFCreateMediaType()?;
    input.CopyAllItems(&output)?;
    let (w, h) = dimensions(input)?;
    for attr in [
        MF_MT_CUSTOM_VIDEO_PRIMARIES,
        MF_MT_MIN_MASTERING_LUMINANCE,
        MF_MT_MAX_MASTERING_LUMINANCE,
        MF_MT_MAX_LUMINANCE_LEVEL,
        MF_MT_MAX_FRAME_AVERAGE_LUMINANCE_LEVEL,
        MF_MT_VIDEO_CHROMA_SITING,
        MF_MT_YUV_MATRIX,
    ] {
        output.DeleteItem(&attr)?;
    }
    output.SetGUID(&MF_MT_SUBTYPE, subtype)?;
    output.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
    output.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
    output.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_0_255.0 as u32)?;
    output.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4)?;
    output.SetUINT32(&MF_MT_SAMPLE_SIZE, w * h * 4)?;
    Ok(output)
}

unsafe fn decoded_input(kind: &IMFMediaType, source: HdrColor) -> Result<Input> {
    if kind.GetGUID(&MF_MT_MAJOR_TYPE)? != MFMediaType_Video || kind.GetGUID(&MF_MT_SUBTYPE)? != MFVideoFormat_P010 {
        return Err(MF_E_INVALIDMEDIATYPE.into());
    }
    dimensions(kind)?;
    let color = HdrColor::from_type(kind, Some(source)).ok_or(MF_E_INVALIDMEDIATYPE)?;
    Ok(Input { kind: kind.clone(), color })
}

unsafe fn manager_device(unknown: &IUnknown) -> Result<ID3D11Device> {
    let manager: IMFDXGIDeviceManager = unknown.cast()?;
    let handle = manager.OpenDeviceHandle()?;
    let mut raw = std::ptr::null_mut();
    let service = manager.GetVideoService(handle, &ID3D11Device::IID, &mut raw);
    let closed = manager.CloseDeviceHandle(handle);
    service?;
    let device = ID3D11Device::from_raw(raw);
    closed?;
    Ok(device)
}

impl Gpu {
    unsafe fn new(device: &ID3D11Device, output: &IMFMediaType) -> Result<Self> {
        let (mut token, mut manager) = (0, None);
        MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
        let manager = manager.ok_or_else(Error::empty)?;
        manager.ResetDevice(device, token)?;
        let mut raw = std::ptr::null_mut();
        MFCreateVideoSampleAllocatorEx(&IMFVideoSampleAllocatorEx::IID, &mut raw)?;
        let allocator = IMFVideoSampleAllocatorEx::from_raw(raw);
        allocator.SetDirectXManager(&manager)?;
        let attrs = attributes()?;
        attrs.SetUINT32(&MF_SA_D3D11_BINDFLAGS, D3D11_BIND_RENDER_TARGET.0 as u32)?;
        allocator.InitializeSampleAllocatorEx(2, 32, &attrs, output)?;
        Ok(Self { device: device.clone(), mapper: ToneMapper::new(device)?, allocator })
    }
}

unsafe fn upload(buffer: &IMFMediaBuffer, input: &IMFMediaType, device: &ID3D11Device) -> Result<ID3D11Texture2D> {
    let (width, height) = dimensions(input)?;
    let stride = if buffer.cast::<IMF2DBuffer>().is_ok() { width * 2 } else { input.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(width * 2) };
    let mut data = std::ptr::null_mut();
    let mut len = 0;
    buffer.Lock(&mut data, None, Some(&mut len))?;
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_P010,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        ..Default::default()
    };
    let mut surface = None;
    let result = if stride < width * 2 || u64::from(stride) * u64::from(height + height.div_ceil(2)) > u64::from(len) {
        Err(MF_E_BUFFERTOOSMALL.into())
    } else {
        let initial = D3D11_SUBRESOURCE_DATA { pSysMem: data.cast(), SysMemPitch: stride, SysMemSlicePitch: 0 };
        device.CreateTexture2D(&desc, Some(&initial), Some(&mut surface))
    };
    buffer.Unlock()?;
    result?;
    surface.ok_or_else(Error::empty)
}

impl State {
    unsafe fn convert(&mut self, fallback_device: &ID3D11Device) -> Result<IMFSample> {
        let input = self.input.as_ref().ok_or(MF_E_TRANSFORM_TYPE_NOT_SET)?;
        let output = self.output.as_ref().ok_or(MF_E_TRANSFORM_TYPE_NOT_SET)?;
        let sample = self.pending.as_ref().ok_or(MF_E_TRANSFORM_NEED_MORE_INPUT)?;
        let buffer = sample.GetBufferByIndex(0)?;
        let (source, subresource) = if let Ok(dxgi) = buffer.cast::<IMFDXGIBuffer>() {
            texture(&dxgi)?
        } else {
            (upload(&buffer, &input.kind, self.managed_device.as_ref().unwrap_or(fallback_device))?, 0)
        };
        let device = source.GetDevice()?;
        if self.gpu.as_ref().is_none_or(|gpu| gpu.device != device) {
            self.gpu = Some(Gpu::new(&device, output)?);
        }
        let gpu = self.gpu.as_mut().ok_or_else(Error::empty)?;
        let result = gpu.allocator.AllocateSample()?;
        let (target, _) = texture(&result.GetBufferByIndex(0)?.cast()?)?;
        let color = input.color;
        gpu.mapper.render(&source, subresource, &target, color.hlg, color.peak_nits, color.full_range)?;
        for attr in [
            MFSampleExtension_Discontinuity,
            MFSampleExtension_CleanPoint,
            MFSampleExtension_Interlaced,
            MFSampleExtension_BottomFieldFirst,
            MFSampleExtension_RepeatFirstField,
            MFSampleExtension_SingleField,
            MFSampleExtension_DerivedFromTopField,
        ] {
            if let Ok(value) = sample.GetUINT32(&attr) {
                result.SetUINT32(&attr, value)?;
            } else {
                result.DeleteItem(&attr)?;
            }
        }
        result.SetSampleFlags(sample.GetSampleFlags()?)?;
        if let Ok(time) = sample.GetSampleTime() {
            result.SetSampleTime(time)?;
        }
        if let Ok(duration) = sample.GetSampleDuration() {
            result.SetSampleDuration(duration)?;
        }
        self.pending = None;
        Ok(result)
    }
}

impl Effect {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn stream(id: u32) -> Result<()> {
    if id == 0 {
        Ok(())
    } else {
        Err(MF_E_INVALIDSTREAMNUMBER.into())
    }
}

impl IMFTransform_Impl for Effect_Impl {
    fn GetStreamLimits(&self, min_in: *mut u32, max_in: *mut u32, min_out: *mut u32, max_out: *mut u32) -> Result<()> {
        for ptr in [min_in, max_in, min_out, max_out] {
            unsafe {
                *ptr.as_mut().ok_or(E_POINTER)? = 1;
            }
        }
        Ok(())
    }

    fn GetStreamCount(&self, inputs: *mut u32, outputs: *mut u32) -> Result<()> {
        unsafe {
            *inputs.as_mut().ok_or(E_POINTER)? = 1;
            *outputs.as_mut().ok_or(E_POINTER)? = 1;
        }
        Ok(())
    }

    fn GetStreamIDs(&self, _: u32, _: *mut u32, _: u32, _: *mut u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GetInputStreamInfo(&self, id: u32, info: *mut MFT_INPUT_STREAM_INFO) -> Result<()> {
        stream(id)?;
        unsafe {
            *info.as_mut().ok_or(E_POINTER)? = MFT_INPUT_STREAM_INFO {
                dwFlags: (MFT_INPUT_STREAM_WHOLE_SAMPLES.0
                    | MFT_INPUT_STREAM_SINGLE_SAMPLE_PER_BUFFER.0
                    | MFT_INPUT_STREAM_FIXED_SAMPLE_SIZE.0
                    | MFT_INPUT_STREAM_HOLDS_BUFFERS.0) as u32,
                ..Default::default()
            };
        }
        Ok(())
    }

    fn GetOutputStreamInfo(&self, id: u32) -> Result<MFT_OUTPUT_STREAM_INFO> {
        stream(id)?;
        Ok(MFT_OUTPUT_STREAM_INFO {
            dwFlags: (MFT_OUTPUT_STREAM_WHOLE_SAMPLES.0
                | MFT_OUTPUT_STREAM_SINGLE_SAMPLE_PER_BUFFER.0
                | MFT_OUTPUT_STREAM_FIXED_SAMPLE_SIZE.0
                | MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0) as u32,
            ..Default::default()
        })
    }

    fn GetAttributes(&self) -> Result<IMFAttributes> {
        Ok(self.attributes.clone())
    }
    fn GetInputStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        stream(id)?;
        Err(E_NOTIMPL.into())
    }
    fn GetOutputStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        stream(id)?;
        Err(E_NOTIMPL.into())
    }
    fn DeleteInputStream(&self, _: u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn AddInputStreams(&self, _: u32, _: *const u32) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GetInputAvailableType(&self, id: u32, index: u32) -> Result<IMFMediaType> {
        stream(id)?;
        if index != 0 {
            return Err(MF_E_NO_MORE_TYPES.into());
        }
        unsafe {
            let ty = MFCreateMediaType()?;
            ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            ty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_P010)?;
            Ok(ty)
        }
    }

    fn GetOutputAvailableType(&self, id: u32, index: u32) -> Result<IMFMediaType> {
        stream(id)?;
        let sub = [MFVideoFormat_ARGB32, MFVideoFormat_RGB32];
        let sub = sub.get(index as usize).ok_or(MF_E_NO_MORE_TYPES)?;
        unsafe { output_type(&self.lock().input.as_ref().ok_or(MF_E_TRANSFORM_TYPE_NOT_SET)?.kind, sub) }
    }

    fn SetInputType(&self, id: u32, ty: Ref<IMFMediaType>, flags: u32) -> Result<()> {
        stream(id)?;
        let mut state = self.lock();
        if state.pending.is_some() {
            return Err(MF_E_TRANSFORM_CANNOT_CHANGE_MEDIATYPE_WHILE_PROCESSING.into());
        }
        let input = ty.as_ref().map(|kind| unsafe { decoded_input(kind, self.color) }).transpose()?;
        if flags & MFT_SET_TYPE_TEST_ONLY.0 as u32 == 0 {
            state.input = input;
            state.output = None;
            state.gpu = None;
        }
        Ok(())
    }

    fn SetOutputType(&self, id: u32, ty: Ref<IMFMediaType>, flags: u32) -> Result<()> {
        stream(id)?;
        let mut state = self.lock();
        if state.pending.is_some() {
            return Err(MF_E_TRANSFORM_CANNOT_CHANGE_MEDIATYPE_WHILE_PROCESSING.into());
        }
        if let Some(ty) = ty.as_ref() {
            let input = state.input.as_ref().ok_or(MF_E_TRANSFORM_TYPE_NOT_SET)?;
            unsafe {
                if ty.GetGUID(&MF_MT_MAJOR_TYPE)? != MFMediaType_Video
                    || ![MFVideoFormat_ARGB32, MFVideoFormat_RGB32].contains(&ty.GetGUID(&MF_MT_SUBTYPE)?)
                    || dimensions(ty)? != dimensions(&input.kind)?
                    || ty.GetUINT32(&MF_MT_VIDEO_PRIMARIES).ok() != Some(MFVideoPrimaries_BT709.0 as u32)
                    || ty.GetUINT32(&MF_MT_TRANSFER_FUNCTION).ok() != Some(MFVideoTransFunc_709.0 as u32)
                {
                    return Err(MF_E_INVALIDMEDIATYPE.into());
                }
            }
        }
        if flags & MFT_SET_TYPE_TEST_ONLY.0 as u32 == 0 {
            state.output = ty.cloned();
            state.gpu = None;
        }
        Ok(())
    }

    fn GetInputCurrentType(&self, id: u32) -> Result<IMFMediaType> {
        stream(id)?;
        self.lock().input.as_ref().map(|input| input.kind.clone()).ok_or_else(|| MF_E_TRANSFORM_TYPE_NOT_SET.into())
    }
    fn GetOutputCurrentType(&self, id: u32) -> Result<IMFMediaType> {
        stream(id)?;
        self.lock().output.clone().ok_or_else(|| MF_E_TRANSFORM_TYPE_NOT_SET.into())
    }
    fn GetInputStatus(&self, id: u32) -> Result<u32> {
        stream(id)?;
        Ok(if self.lock().pending.is_none() { MFT_INPUT_STATUS_ACCEPT_DATA.0 as u32 } else { 0 })
    }
    fn GetOutputStatus(&self) -> Result<u32> {
        Ok(if self.lock().pending.is_some() { MFT_OUTPUT_STATUS_SAMPLE_READY.0 as u32 } else { 0 })
    }
    fn SetOutputBounds(&self, _: i64, _: i64) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ProcessEvent(&self, id: u32, _: Ref<IMFMediaEvent>) -> Result<()> {
        stream(id)?;
        Err(E_NOTIMPL.into())
    }

    fn ProcessMessage(&self, message: MFT_MESSAGE_TYPE, param: usize) -> Result<()> {
        let mut state = self.lock();
        match message {
            MFT_MESSAGE_COMMAND_FLUSH | MFT_MESSAGE_NOTIFY_END_STREAMING => {
                state.pending = None;
            }
            MFT_MESSAGE_SET_D3D_MANAGER => {
                let raw = param as *mut _;
                state.managed_device = unsafe { IUnknown::from_raw_borrowed(&raw).map(|unk| manager_device(unk)).transpose()? };
                state.gpu = None;
            }
            _ => {}
        }
        Ok(())
    }

    fn ProcessInput(&self, id: u32, sample: Ref<IMFSample>, flags: u32) -> Result<()> {
        stream(id)?;
        if flags != 0 {
            return Err(E_INVALIDARG.into());
        }
        let mut state = self.lock();
        if state.output.is_none() {
            return Err(MF_E_TRANSFORM_TYPE_NOT_SET.into());
        }
        if state.pending.is_some() {
            return Err(MF_E_NOTACCEPTING.into());
        }
        state.pending = Some(sample.ok()?.clone());
        Ok(())
    }

    fn ProcessOutput(&self, flags: u32, count: u32, buffers: *mut MFT_OUTPUT_DATA_BUFFER, status: *mut u32) -> Result<()> {
        if flags != 0 || count != 1 {
            return Err(E_INVALIDARG.into());
        }
        unsafe {
            *status.as_mut().ok_or(E_POINTER)? = 0;
            let buffer = buffers.as_mut().ok_or(E_POINTER)?;
            stream(buffer.dwStreamID)?;
            if buffer.pSample.is_some() {
                return Err(E_INVALIDARG.into());
            }
            buffer.dwStatus = 0;
            buffer.pEvents = ManuallyDrop::new(None);
            buffer.pSample = ManuallyDrop::new(Some(self.lock().convert(&self.device)?));
        }
        Ok(())
    }
}
