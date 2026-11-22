use std::ffi::CStr;
use std::ops::ControlFlow;

pub(crate) fn for_each(mut buf: &[u8], mut each: impl FnMut(&CStr, u8, u64) -> bool) -> Option<ControlFlow<()>> {
    while let Some(header) = buf.first_chunk::<20>() {
        let len = usize::from(u16::from_ne_bytes([header[16], header[17]]));
        let record = buf.get(..len).filter(|r| r.len() >= 20)?;
        let ino = u64::from_ne_bytes(header[..8].try_into().ok()?);
        let Ok(name) = CStr::from_bytes_until_nul(&record[19..]) else { break };
        buf = &buf[len..];
        if !matches!(name.to_bytes(), b"." | b"..") && !each(name, header[18], ino) {
            return Some(ControlFlow::Break(()));
        }
    }
    Some(ControlFlow::Continue(()))
}
