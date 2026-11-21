use windows::core::{Error, Interface, Result};
use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Direct3D::{D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D11_SRV_DIMENSION_TEXTURE2D, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_UNSUPPORTED;

const VS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/hdr_vs.cso"));
const PS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/hdr_ps.cso"));

struct Planes {
    size: (u32, u32),
    texture: ID3D11Texture2D,
    views: [Option<ID3D11ShaderResourceView>; 2],
}

impl Planes {
    unsafe fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_P010,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let mut texture = None;
        device.CreateTexture2D(&desc, None, Some(&mut texture))?;
        let texture = texture.unwrap();
        let mut views = [None, None];
        for (slot, format) in views.iter_mut().zip([DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM]) {
            let view = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: format,
                ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } },
            };
            device.CreateShaderResourceView(&texture, Some(&view), Some(slot))?;
        }
        Ok(Self { size: (width, height), texture, views })
    }
}

pub(crate) struct ToneMapper {
    device: ID3D11Device,
    immediate: ID3D11DeviceContext,
    deferred: ID3D11DeviceContext,
    multithread: ID3D10Multithread,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    constants: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    planes: Option<Planes>,
}

impl ToneMapper {
    pub(crate) fn check_support(device: &ID3D11Device) -> Result<()> {
        unsafe {
            if device.GetFeatureLevel().0 < D3D_FEATURE_LEVEL_11_0.0 {
                return Err(Error::from_hresult(DXGI_ERROR_UNSUPPORTED));
            }
            Planes::new(device, 2, 2)?;
        }
        Ok(())
    }

    pub(crate) fn new(device: &ID3D11Device) -> Result<Self> {
        unsafe {
            let (mut vs, mut ps, mut deferred, mut constants, mut sampler) = (None, None, None, None, None);
            device.CreateVertexShader(VS, None, Some(&mut vs))?;
            device.CreatePixelShader(PS, None, Some(&mut ps))?;
            device.CreateDeferredContext(0, Some(&mut deferred))?;
            let buffer = D3D11_BUFFER_DESC { ByteWidth: 16, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
            device.CreateBuffer(&buffer, None, Some(&mut constants))?;
            let desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            device.CreateSamplerState(&desc, Some(&mut sampler))?;
            Ok(Self {
                device: device.clone(),
                immediate: device.GetImmediateContext()?,
                deferred: deferred.unwrap(),
                multithread: device.cast()?,
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                constants: constants.unwrap(),
                sampler: sampler.unwrap(),
                planes: None,
            })
        }
    }

    pub(crate) fn render(&mut self, input: &ID3D11Texture2D, subresource: u32, output: &ID3D11Texture2D, hlg: bool, peak_nits: f32, full_range: bool) -> Result<()> {
        unsafe {
            let (mut src, mut dst) = (D3D11_TEXTURE2D_DESC::default(), D3D11_TEXTURE2D_DESC::default());
            input.GetDesc(&mut src);
            output.GetDesc(&mut dst);
            if src.Format != DXGI_FORMAT_P010 || src.MipLevels != 1 || subresource >= src.ArraySize || dst.Width > src.Width || dst.Height > src.Height {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            if self.planes.as_ref().is_none_or(|p| p.size != (src.Width, src.Height)) {
                self.planes = Some(Planes::new(&self.device, src.Width, src.Height)?);
            }
            let planes = self.planes.as_ref().unwrap();
            let mut target = None;
            self.device.CreateRenderTargetView(output, None, Some(&mut target))?;
            let ctx = &self.deferred;
            let color = [u32::from(hlg) as f32, (peak_nits / 100.0).clamp(1.0, 100.0), u32::from(full_range) as f32, 0.0];
            ctx.UpdateSubresource(&self.constants, 0, None, color.as_ptr().cast(), 0, 0);
            ctx.CopySubresourceRegion(&planes.texture, 0, 0, 0, 0, input, subresource, None);
            ctx.OMSetRenderTargets(Some(&[target]), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { Width: dst.Width as f32, Height: dst.Height as f32, MaxDepth: 1.0, ..Default::default() }]));
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            ctx.PSSetShaderResources(0, Some(&planes.views));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.Draw(3, 0);
            let mut list = None;
            ctx.FinishCommandList(false, Some(&mut list))?;
            self.multithread.Enter();
            self.immediate.ExecuteCommandList(list.as_ref().unwrap(), true);
            self.multithread.Leave();
            Ok(())
        }
    }
}
