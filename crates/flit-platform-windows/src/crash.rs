use std::ffi::c_void;
use std::fmt::{self, Write as _};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use windows::core::{s, w, BOOL, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, HMODULE};
use windows::Win32::Storage::FileSystem::{CreateFileW, WriteFile, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE};
use windows::Win32::System::Diagnostics::Debug::{SetUnhandledExceptionFilter, EXCEPTION_POINTERS};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW, GetProcAddress, LoadLibraryExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows::Win32::System::Recovery::{RegisterApplicationRestart, RESTART_NO_CRASH, RESTART_NO_HANG};
use windows::Win32::System::SystemInformation::GetSystemTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};

pub const PREFIX: &str = "flit-crash-";
pub const KEEP: usize = 10;
pub const REPORT_SUFFIX: &str = ".report.txt";

const DUMP_FLAGS: u32 = 0x0000_0040 | 0x0000_1000 | 0x0000_0020 | 0x0000_0004;
const EXECUTE_HANDLER: i32 = 1;
const PATH_MAX: usize = 1024;
const RAW_SUFFIX: &str = ".raw";

#[repr(C, packed(4))]
struct ExceptionInfo {
    thread_id: u32,
    pointers: *const EXCEPTION_POINTERS,
    client_pointers: BOOL,
}

type WriteDump = unsafe extern "system" fn(HANDLE, u32, HANDLE, u32, *const ExceptionInfo, *const c_void, *const c_void) -> BOOL;

static CAPTURE: OnceLock<Capture> = OnceLock::new();
static DUMPING: AtomicBool = AtomicBool::new(false);

struct Capture {
    base: Vec<u16>,
    os: String,
    dump: Option<WriteDump>,
}

struct Report {
    dir: PathBuf,
    home: String,
    user: String,
    os: String,
}

pub fn install(dir: &Path) {
    let home = <crate::Windows as flit_platform::Platform>::home();
    install_report(Report {
        dir: dir.to_owned(),
        user: std::env::var("USERNAME").unwrap_or_else(|_| flit_core::name::display(home.file_name()).into_owned()),
        home: home.display().into_owned(),
        os: crate::places::os_version(),
    });
}

fn install_report(report: Report) {
    let dir = &report.dir;
    let _ = std::fs::create_dir_all(dir);
    prune(dir, KEEP);
    report.finish();
    let base: Vec<u16> = dir.join(PREFIX).as_os_str().encode_wide().collect();
    if base.len() + 64 < PATH_MAX && CAPTURE.get().is_none() {
        let _ = CAPTURE.set(Capture { base, os: report.os, dump: load_dump() });
        rearm();
    }
}

fn load_dump() -> Option<WriteDump> {
    unsafe {
        let library = LoadLibraryExW(w!("dbghelp.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        let function = GetProcAddress(library, s!("MiniDumpWriteDump"))?;
        Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, WriteDump>(function))
    }
}

pub(crate) fn rearm() {
    if CAPTURE.get().is_some() {
        unsafe {
            SetUnhandledExceptionFilter(Some(filter));
        }
    }
}

pub fn register_restart(arg: &str) {
    let line: Vec<u16> = arg.encode_utf16().chain(Some(0)).collect();
    if let Err(e) = unsafe { RegisterApplicationRestart(PCWSTR(line.as_ptr()), RESTART_NO_CRASH | RESTART_NO_HANG) } {
        tracing::info!("cannot register for a restart after Windows updates: {e}");
    }
}

pub fn dump_now(panic: &str) {
    unsafe { write(std::ptr::null(), panic) }
}

pub fn prune(dir: &Path, keep: usize) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut crashes: std::collections::HashMap<String, std::time::SystemTime> = Default::default();
    for entry in read.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())) {
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| name.starts_with(PREFIX)) else { continue };
        let Some(stem) = [".dmp", RAW_SUFFIX, REPORT_SUFFIX].into_iter().find_map(|suffix| name.strip_suffix(suffix)) else { continue };
        let time = entry.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        crashes.entry(stem.to_owned()).and_modify(|t| *t = (*t).min(time)).or_insert(time);
    }
    let mut crashes: Vec<_> = crashes.into_iter().collect();
    crashes.sort_unstable_by(|a, b| (&b.1, &b.0).cmp(&(&a.1, &a.0)));
    for (stem, _) in crashes.into_iter().skip(keep) {
        for suffix in [RAW_SUFFIX, REPORT_SUFFIX, ".txt", ".dmp"] {
            let _ = std::fs::remove_file(dir.join(format!("{stem}{suffix}")));
        }
    }
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    write(info, "");
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

    fn suffix(&mut self, stem: usize, ext: &str) {
        self.len = stem;
        ext.bytes().for_each(|b| self.push(&[u16::from(b)]));
        self.buf[self.len] = 0;
    }
}

unsafe fn create(name: &Name) -> Option<HANDLE> {
    CreateFileW(PCWSTR(name.buf.as_ptr()), GENERIC_WRITE.0, FILE_SHARE_NONE, None, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, None).ok()
}

unsafe fn write(info: *const EXCEPTION_POINTERS, panic: &str) {
    let Some(capture) = CAPTURE.get() else { return };
    let t = GetSystemTime();
    let mut name = Name { buf: [0; PATH_MAX], len: 0 };
    name.push(&capture.base);
    name.num(t.wYear.into(), 4);
    name.num(t.wMonth.into(), 2);
    name.num(t.wDay.into(), 2);
    name.push(&[b'-' as u16]);
    name.num(t.wHour.into(), 2);
    name.num(t.wMinute.into(), 2);
    name.num(t.wSecond.into(), 2);
    name.num(t.wMilliseconds.into(), 3);
    name.push(&[b'-' as u16]);
    let (pid, tid) = (GetCurrentProcessId(), GetCurrentThreadId());
    name.num(pid, 1);
    name.push(&[b'-' as u16]);
    name.num(tid, 1);
    let stem = name.len;
    name.suffix(stem, RAW_SUFFIX);
    if let Some(file) = create(&name) {
        let mut facts = Facts(file);
        let _ = write!(facts, concat!("Flit ", env!("CARGO_PKG_VERSION"), "\nOS: {}\n\n"), capture.os);
        if info.is_null() {
            let _ = facts.write_str(panic);
        } else {
            describe(&mut facts, info);
        }
        let _ = CloseHandle(file);
    }
    if let Some(write_dump) = capture.dump.filter(|_| !DUMPING.swap(true, Ordering::Acquire)) {
        name.suffix(stem, ".dmp");
        if let Some(file) = create(&name) {
            let exception = ExceptionInfo { thread_id: tid, pointers: info, client_pointers: false.into() };
            let _ = write_dump(GetCurrentProcess(), pid, file, DUMP_FLAGS, if info.is_null() { std::ptr::null() } else { &exception }, std::ptr::null(), std::ptr::null());
            let _ = CloseHandle(file);
        }
        DUMPING.store(false, Ordering::Release);
    }
}

struct Facts(HANDLE);

impl Facts {
    fn bytes(&mut self, bytes: &[u8]) -> fmt::Result {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut len = 0;
        unsafe { WriteFile(self.0, Some(bytes), Some(&mut len), None) }.map_err(|_| fmt::Error)?;
        if len as usize == bytes.len() { Ok(()) } else { Err(fmt::Error) }
    }

    fn wide(&mut self, units: &[u16]) -> fmt::Result {
        let mut buf = [0u8; 256];
        let mut len = 0;
        for ch in char::decode_utf16(units.iter().copied()) {
            if len + 4 > buf.len() {
                self.bytes(&buf[..len])?;
                len = 0;
            }
            len += ch.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut buf[len..]).len();
        }
        self.bytes(&buf[..len])
    }
}

impl fmt::Write for Facts {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.bytes(text.as_bytes())
    }
}

unsafe fn describe(facts: &mut Facts, info: *const EXCEPTION_POINTERS) {
    let Some(record) = info.as_ref().and_then(|i| i.ExceptionRecord.as_ref()) else { return };
    let at = record.ExceptionAddress;
    let _ = write!(facts, "exception 0x{:08X} at {at:?}\nmodule ", record.ExceptionCode.0 as u32);
    let mut buf = [0u16; PATH_MAX];
    if let Some(module) = module_at(at) {
        let _ = facts.wide(module_path(module, &mut buf));
    } else {
        let _ = facts.write_str("unknown");
    }
    let _ = facts.write_char('\n');
    #[cfg(target_arch = "x86_64")]
    if let Some(context) = info.as_ref().and_then(|i| i.ContextRecord.as_ref()) {
        stack(facts, *context, &mut buf);
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn stack(facts: &mut Facts, mut context: windows::Win32::System::Diagnostics::Debug::CONTEXT, buf: &mut [u16]) {
    use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, RtlLookupFunctionEntry, RtlVirtualUnwind, UNW_FLAG_NHANDLER};
    use windows::Win32::System::Threading::GetCurrentThreadStackLimits;
    let (mut low, mut high) = (0, 0);
    GetCurrentThreadStackLimits(&mut low, &mut high);
    let _ = facts.write_str("Stack:\n");
    for frame in 0..48 {
        let (pc, sp) = (context.Rip, context.Rsp);
        if pc == 0 || !(low..high).contains(&(sp as usize)) {
            break;
        }
        let mut base = 0;
        let function = RtlLookupFunctionEntry(pc, &mut base, None);
        let module = module_at(pc as *const c_void);
        let _ = write!(facts, "#{frame} ");
        if let Some(module) = module {
            let path = module_path(module, buf);
            let name = path.rsplit(|&c| c == u16::from(b'\\') || c == u16::from(b'/')).next().unwrap_or_default();
            let _ = facts.wide(name);
        }
        let offset = if base == 0 { module.map_or(0, |m| m.0 as u64) } else { base };
        let _ = writeln!(facts, "+0x{:X} (0x{pc:X})", pc.saturating_sub(offset));
        if function.is_null() {
            let mut next = 0u64;
            if ReadProcessMemory(GetCurrentProcess(), sp as *const c_void, (&mut next as *mut u64).cast(), size_of::<u64>(), None).is_err() {
                break;
            }
            context.Rip = next;
            context.Rsp += size_of::<u64>() as u64;
        } else {
            let _ = RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, pc, function, &mut context, &mut std::ptr::null_mut(), &mut 0, None);
        }
        if context.Rsp <= sp {
            break;
        }
    }
}

impl Report {
    fn finish(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        for entry in entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())) {
            let name = entry.file_name();
            let Some(stem) = name.to_str().filter(|name| name.starts_with(PREFIX)).and_then(|name| name.strip_suffix(RAW_SUFFIX)) else { continue };
            let raw = entry.path();
            let path = self.dir.join(format!("{stem}{REPORT_SUFFIX}"));
            if !path.is_file() {
                let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
                let Ok(facts) = std::fs::read_to_string(&raw) else { continue };
                let temp = raw.with_extension(format!("{}.tmp", std::process::id()));
                let result = (|| {
                    use std::io::Write as _;
                    let mut file = std::fs::File::create(&temp)?;
                    file.write_all(self.text(&facts).as_bytes())?;
                    file.set_modified(modified)?;
                    drop(file);
                    std::fs::rename(&temp, &path)
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(temp);
                    continue;
                }
            }
            let _ = std::fs::remove_file(raw);
        }
    }

    fn text(&self, diagnostic: &str) -> String {
        let mut recent = Vec::new();
        for file in [flit_core::ui::LOG_FILE, "flit.1.log"] {
            let Ok(bytes) = std::fs::read(self.dir.join(file)) else { continue };
            let log = String::from_utf8_lossy(&bytes);
            recent.extend(log.lines().rev().take(50 - recent.len()).map(str::to_owned));
            if recent.len() == 50 {
                break;
            }
        }
        recent.reverse();
        let text = format!("{diagnostic}\nRecent log:\n{}\n", recent.join("\n"));
        flit_core::redact::crash_text(&text, &self.home, &self.user, true)
    }
}

unsafe fn module_at(at: *const c_void) -> Option<HMODULE> {
    let mut module = HMODULE::default();
    GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, PCWSTR(at.cast()), &mut module).ok()?;
    Some(module)
}

unsafe fn module_path(module: HMODULE, buf: &mut [u16]) -> &[u16] {
    let n = GetModuleFileNameW(Some(module), buf) as usize;
    &buf[..n]
}

#[cfg(test)]
#[path = "../test/crash.rs"]
mod tests;
