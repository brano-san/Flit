use super::*;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;

#[test]
#[ignore = "requires D3D11 with P010 plane views"]
fn software_frames_reuse_the_upload_texture_and_pooled_render_target_views() {
    unsafe {
        let mut device = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, None).unwrap();
        let device = device.unwrap();
        let mut mapper = ToneMapper::new(&device).unwrap();
        let desc = D3D11_TEXTURE2D_DESC { Width: 4, Height: 4, ArraySize: 2, MipLevels: 1, Format: DXGI_FORMAT_B8G8R8A8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default() };
        let mut output = None;
        device.CreateTexture2D(&desc, None, Some(&mut output)).unwrap();
        let output = output.unwrap();
        let mut pixels = [512u16 << 6; 24];
        pixels[..16].fill(64 << 6);
        let first = mapper.upload(4, 4, pixels.as_ptr().cast(), 8).unwrap();
        mapper.render(&first, 0, (&output, 0), false, 1000.0, false).unwrap();
        let black_view = mapper.target_view(&output, 0, &desc).unwrap();
        pixels[..16].fill(940 << 6);
        let second = mapper.upload(4, 4, pixels.as_ptr().cast(), 8).unwrap();
        assert_eq!(first, second);
        mapper.render(&second, 0, (&output, 1), false, 1000.0, false).unwrap();
        assert_eq!(black_view, mapper.target_view(&output, 0, &desc).unwrap());
        assert_ne!(black_view, mapper.target_view(&output, 1, &desc).unwrap());

        let staging_desc = D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, BindFlags: 0, ..desc };
        let mut staging = None;
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging)).unwrap();
        let staging = staging.unwrap();
        let ctx = device.GetImmediateContext().unwrap();
        ctx.CopyResource(&staging, &output);
        for (slice, expected) in [(0, 0), (1, 255)] {
            let mut mapping = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, slice, D3D11_MAP_READ, 0, Some(&mut mapping)).unwrap();
            let pixel = *mapping.pData.cast::<[u8; 4]>();
            ctx.Unmap(&staging, slice);
            assert_eq!(pixel, [expected, expected, expected, 255]);
        }
    }
}
