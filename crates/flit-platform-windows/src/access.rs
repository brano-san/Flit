use crate::walk_index::Handle;
use flit_core::FsPath;
use flit_search::engine::Visibility;
use flit_search::index::{VolumeIndex, NONE};
use std::collections::HashMap;
use std::io;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::*;
use windows::Win32::Storage::FileSystem::{FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_LIST_DIRECTORY};
use windows::Win32::System::Pipes::ImpersonateNamedPipeClient;
use windows::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

const FORGET_AFTER: Duration = Duration::from_secs(300);
const ROOT_KEY: u64 = u64::MAX;

type Cache = HashMap<String, HashMap<u64, bool>>;

pub(crate) struct TokenAccess {
    token: Handle,
    cache: RwLock<(Instant, Cache)>,
}

impl TokenAccess {
    pub fn new(token: Handle) -> Self {
        Self { token, cache: RwLock::new((Instant::now(), Cache::new())) }
    }

    pub fn of_pipe_client(pipe: HANDLE) -> io::Result<Self> {
        unsafe { ImpersonateNamedPipeClient(pipe) }.map_err(io::Error::other)?;
        let mut token = HANDLE::default();
        let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token) };
        if unsafe { RevertToSelf() }.is_err() {
            std::process::abort();
        }
        opened.map_err(io::Error::other)?;
        Ok(Self::new(Handle(token)))
    }

    fn check(&self, path: &[u16]) -> bool {
        let info = (OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION).0;
        let mut need = 0u32;
        let _ = unsafe { GetFileSecurityW(PCWSTR(path.as_ptr()), info, None, 0, &mut need) };
        if need == 0 {
            return false;
        }
        let mut sd = vec![0u64; (need as usize).div_ceil(8)];
        let sd = PSECURITY_DESCRIPTOR(sd.as_mut_ptr() as _);
        if !unsafe { GetFileSecurityW(PCWSTR(path.as_ptr()), info, Some(sd), need, &mut need) }.as_bool() {
            return false;
        }
        let mapping = GENERIC_MAPPING { GenericRead: FILE_GENERIC_READ.0, GenericWrite: FILE_GENERIC_WRITE.0, GenericExecute: FILE_GENERIC_EXECUTE.0, GenericAll: FILE_ALL_ACCESS.0 };
        let mut privileges = PRIVILEGE_SET::default();
        let mut privileges_len = size_of::<PRIVILEGE_SET>() as u32;
        let (mut granted, mut status) = (0u32, BOOL(0));
        let checked = unsafe { AccessCheck(sd, self.token.0, FILE_LIST_DIRECTORY.0, &mapping, Some(&mut privileges), &mut privileges_len, &mut granted, &mut status) };
        checked.is_ok() && status.as_bool()
    }
}

impl Visibility for TokenAccess {
    fn can_list(&self, v: &VolumeIndex, dir: u32) -> bool {
        let key = if dir == NONE { ROOT_KEY } else { v.ids[dir as usize] };
        {
            let cache = self.cache.read().unwrap_or_else(|e| e.into_inner());
            if cache.0.elapsed() < FORGET_AFTER {
                if let Some(&known) = cache.1.get(v.id.as_str()).and_then(|m| m.get(&key)) {
                    return known;
                }
            }
        }
        let mut bytes = Vec::new();
        match dir {
            NONE => bytes.extend_from_slice(&v.root),
            _ => v.path_into(dir, false, &mut Vec::new(), &mut bytes),
        }
        let allowed = FsPath::parse_bytes(&bytes).is_some_and(|p| self.check(&p.to_wide(true)));
        let mut cache = self.cache.write().unwrap_or_else(|e| e.into_inner());
        if cache.0.elapsed() >= FORGET_AFTER {
            *cache = (Instant::now(), Cache::new());
        }
        cache.1.entry(v.id.clone()).or_default().insert(key, allowed);
        allowed
    }
}

#[cfg(test)]
#[path = "../test/access.rs"]
mod tests;
