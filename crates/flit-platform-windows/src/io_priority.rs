use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{IoPriorityHintVeryLow, FILE_IO_PRIORITY_HINT_INFO};

#[repr(C, align(8))]
struct AlignedHint(FILE_IO_PRIORITY_HINT_INFO);

pub(crate) fn idle(handle: HANDLE) {
    if handle.is_invalid() {
        return;
    }
    let hint = AlignedHint(FILE_IO_PRIORITY_HINT_INFO { PriorityHint: IoPriorityHintVeryLow });
    apply(handle, &hint);
}

#[cfg(not(test))]
fn apply(handle: HANDLE, hint: &AlignedHint) {
    use windows::Win32::Storage::FileSystem::{FileIoPriorityHintInfo, SetFileInformationByHandle};
    let _ = unsafe { SetFileInformationByHandle(handle, FileIoPriorityHintInfo, (&hint.0 as *const FILE_IO_PRIORITY_HINT_INFO).cast(), size_of::<FILE_IO_PRIORITY_HINT_INFO>() as u32) };
}

#[cfg(test)]
#[path = "../test/io_priority.rs"]
mod tests;
#[cfg(test)]
use tests::apply;
