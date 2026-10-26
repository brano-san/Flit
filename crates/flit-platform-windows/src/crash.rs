use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use windows::core::{s, w, BOOL, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, HMODULE};
use windows::Win32::Storage::FileSystem::{CreateFileW, WriteFile, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE};
use windows::Win32::System::Diagnostics::Debug::{SetUnhandledExceptionFilter, EXCEPTION_POINTERS};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW, GetProcAddress, LoadLibraryExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows::Win32::System::SystemInformation::GetSystemTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};

pub const PREFIX: &str = "flit-crash-";
pub const KEEP: usize = 5;

const DUMP_FLAGS: u32 = 0x0000_0040 | 0x0000_1000 | 0x0000_0020 | 0x0000_0004;
const EXECUTE_HANDLER: i32 = 1;
const PATH_MAX: usize = 1024;

#[repr(C, packed(4))]
struct ExceptionInfo {
    thread_id: u32,
    pointers: *const EXCEPTION_POINTERS,
    client_pointers: BOOL,
}

type WriteDump = unsafe extern "system" fn(HANDLE, u32, HANDLE, u32, *const ExceptionInfo, *const c_void, *const c_void) -> BOOL;

static BASE: OnceLock<Vec<u16>> = OnceLock::new();

pub fn install(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    prune(dir, KEEP);
    let base: Vec<u16> = dir.join(PREFIX).as_os_str().encode_wide().collect();
    if base.len() + 64 < PATH_MAX && BASE.set(base).is_ok() {
        rearm();
    }
}

pub(crate) fn rearm() {
    if BASE.get().is_some() {
        unsafe {
            SetUnhandledExceptionFilter(Some(filter));
        }
    }
}

pub fn dump_now() -> Option<PathBuf> {
    unsafe { write(std::ptr::null()) }
}

pub fn prune(dir: &Path, keep: usize) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut dumps: Vec<(std::time::SystemTime, PathBuf)> = read
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(PREFIX) && e.path().extension().is_some_and(|x| x == "dmp"))
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.path()))
        .collect();
    dumps.sort_by_key(|d| std::cmp::Reverse(d.0));
    for (_, p) in dumps.into_iter().skip(keep) {
        let _ = std::fs::remove_file(p.with_extension("txt"));
        let _ = std::fs::remove_file(p);
    }
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    write(info);
    EXECUTE_HANDLER
}

struct Name {
    buf: [u16; PATH_MAX],
    len: usize,
}

impl Name {
    fn push(&mut self, units: &[u16]) {
        let n = units.len().min(PATH_MAX - 1 - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&units[..n]);
        self.len += n;
    }

    fn num(&mut self, v: u32, width: usize) {
        let mut digits = [b'0' as u16; 10];
        let mut v = v;
        for d in digits.iter_mut().rev() {
            *d = b'0' as u16 + (v % 10) as u16;
            v /= 10;
        }
        let width = width.max(10 - digits.iter().position(|&d| d != b'0' as u16).unwrap_or(9));
        self.push(&digits[10 - width.min(10)..]);
    }

    fn with(&self, ext: &str) -> Name {
        let mut n = Name { buf: self.buf, len: self.len };
        ext.bytes().for_each(|b| n.push(&[u16::from(b)]));
        n.buf[n.len] = 0;
        n
    }

    fn path(&self) -> PathBuf {
        PathBuf::from(String::from_utf16_lossy(&self.buf[..self.len]))
    }
}

unsafe fn create(name: &Name) -> Option<HANDLE> {
    CreateFileW(PCWSTR(name.buf.as_ptr()), GENERIC_WRITE.0, FILE_SHARE_NONE, None, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, None).ok()
}

unsafe fn write(info: *const EXCEPTION_POINTERS) -> Option<PathBuf> {
    let base = BASE.get()?;
    let t = GetSystemTime();
    let mut name = Name { buf: [0; PATH_MAX], len: 0 };
    name.push(base);
    name.num(t.wYear.into(), 4);
    name.num(t.wMonth.into(), 2);
    name.num(t.wDay.into(), 2);
    name.push(&[b'-' as u16]);
    name.num(t.wHour.into(), 2);
    name.num(t.wMinute.into(), 2);
    name.num(t.wSecond.into(), 2);
    name.push(&[b'-' as u16]);
    name.num(GetCurrentProcessId(), 1);
    let dbghelp = LoadLibraryExW(w!("dbghelp.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
    let write_dump: WriteDump = std::mem::transmute(GetProcAddress(dbghelp, s!("MiniDumpWriteDump"))?);
    let dump = name.with(".dmp");
    let file = create(&dump)?;
    let exception = ExceptionInfo { thread_id: GetCurrentThreadId(), pointers: info, client_pointers: false.into() };
    let ok = write_dump(GetCurrentProcess(), GetCurrentProcessId(), file, DUMP_FLAGS, if info.is_null() { std::ptr::null() } else { &exception }, std::ptr::null(), std::ptr::null()).as_bool();
    let _ = CloseHandle(file);
    if let Some(note) = create(&name.with(".txt")) {
        let text = describe(info);
        let _ = WriteFile(note, Some(text.as_bytes()), None, None);
        let _ = CloseHandle(note);
    }
    ok.then(|| dump.path())
}

unsafe fn describe(info: *const EXCEPTION_POINTERS) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let Some(record) = info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) else { return format!("Flit {version}\npanic\n") };
    let at = record.ExceptionAddress;
    format!("Flit {version}\nexception 0x{:08X} at {at:?}\nmodule {}\n", record.ExceptionCode.0 as u32, module_of(at).map_or_else(|| "unknown".into(), |p| p.display().to_string()))
}

pub fn module_of(at: *const c_void) -> Option<PathBuf> {
    let mut module = HMODULE::default();
    unsafe {
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, PCWSTR(at as *const u16), &mut module).ok()?;
        let mut buf = [0u16; PATH_MAX];
        let n = GetModuleFileNameW(Some(module), &mut buf) as usize;
        (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}

#[cfg(test)]
#[path = "../test/crash.rs"]
mod tests;
