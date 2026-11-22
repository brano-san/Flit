use super::*;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

struct Foundation;

impl Foundation {
    unsafe fn start() -> Self {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
        MFStartup(MF_VERSION, MFSTARTUP_FULL).unwrap();
        Self
    }
}

impl Drop for Foundation {
    fn drop(&mut self) {
        unsafe {
            MFShutdown().unwrap();
            CoUninitialize();
        }
    }
}

unsafe fn video(subtype: GUID, primaries: MFVideoPrimaries, transfer: MFVideoTransferFunction) -> IMFMediaType {
    let kind = MFCreateMediaType().unwrap();
    kind.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
    kind.SetGUID(&MF_MT_SUBTYPE, &subtype).unwrap();
    kind.SetUINT64(&MF_MT_FRAME_SIZE, (640 << 32) | 360).unwrap();
    kind.SetUINT32(&MF_MT_VIDEO_PRIMARIES, primaries.0 as u32).unwrap();
    kind.SetUINT32(&MF_MT_TRANSFER_FUNCTION, transfer.0 as u32).unwrap();
    kind
}

#[test]
fn only_decoded_hdr_video_is_tone_mapped() {
    unsafe {
        let _mf = Foundation::start();
        for (subtype, primaries, transfer) in [
            (MFVideoFormat_NV12, MFVideoPrimaries_BT709, MFVideoTransFunc_709),
            (MFVideoFormat_P010, MFVideoPrimaries_BT709, MFVideoTransFunc_709),
            (MFVideoFormat_NV12, MFVideoPrimaries_SMPTE170M, MFVideoTransFunc_709),
            (MFVideoFormat_P010, MFVideoPrimaries_BT2020, MFVideoTransFunc_709),
            (MFVideoFormat_P010, MFVideoPrimaries_BT2020, MFVideoTransferFunction(0)),
            (MFVideoFormat_P010, MFVideoPrimaries_BT2020, MFVideoTransferFunction(99)),
            (MFVideoFormat_HEVC, MFVideoPrimaries_BT2020, MFVideoTransFunc_2084),
        ] {
            let kind = video(subtype, primaries, transfer);
            assert!(HdrColor::from_type(&kind).is_none(), "{subtype:?} {primaries:?} {transfer:?}");
        }
        for (transfer, hlg) in [(MFVideoTransFunc_2084, false), (MFVideoTransFunc_HLG, true)] {
            let kind = video(MFVideoFormat_P010, MFVideoPrimaries_BT2020, transfer);
            kind.SetUINT32(&MF_MT_MAX_LUMINANCE_LEVEL, 4000).unwrap();
            let limited = HdrColor::from_type(&kind).unwrap();
            assert_eq!(limited.hlg, hlg);
            assert_eq!(limited.peak_nits, 4000.0);
            assert!(!limited.full_range);
            kind.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_0_255.0 as u32).unwrap();
            assert!(HdrColor::from_type(&kind).unwrap().full_range);
        }
    }
}

#[test]
fn unchanged_video_returns_the_decoders_original_sample() {
    unsafe {
        let _mf = Foundation::start();
        for (format, primaries, transfer) in [(MFVideoFormat_NV12, MFVideoPrimaries_BT709, MFVideoTransFunc_709), (MFVideoFormat_P010, MFVideoPrimaries_BT2020, MFVideoTransFunc_709), (MFVideoFormat_P010, MFVideoPrimaries_Unknown, MFVideoTransFunc_Unknown)] {
            let kind = video(format, primaries, transfer);
            let input = Input::decoded(&kind).unwrap();
            let output = input.output_type(0).unwrap();
            assert!(kind.Compare(&output, MF_ATTRIBUTES_MATCH_ALL_ITEMS).unwrap().as_bool());
            let sample = MFCreateSample().unwrap();
            let mut state = State { input: Some(input), output: Some(output.clone()), pending: Some(sample.clone()), ..Default::default() };
            assert_eq!(state.take_passthrough().unwrap(), sample);
            assert!(state.pending.is_none());
            let tagged = Input::decoded(&video(MFVideoFormat_P010, MFVideoPrimaries_BT2020, MFVideoTransFunc_2084)).unwrap();
            assert!(!tagged.accepts_output(&output).unwrap());
            assert_eq!(tagged.output_type(0).unwrap().GetGUID(&MF_MT_SUBTYPE).unwrap(), MFVideoFormat_ARGB32);
        }
    }
}
