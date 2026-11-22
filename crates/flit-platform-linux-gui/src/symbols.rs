use std::ffi::{c_void, CStr};
use std::ptr::NonNull;

pub(crate) struct Library(NonNull<c_void>);

impl Library {
    pub(crate) fn open(name: &CStr) -> Option<Self> {
        NonNull::new(unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL | libc::RTLD_NODELETE) }).map(Self)
    }

    pub(crate) fn symbol(&self, name: &CStr) -> Option<NonNull<c_void>> {
        NonNull::new(unsafe { libc::dlsym(self.0.as_ptr(), name.as_ptr()) })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { libc::dlclose(self.0.as_ptr()) };
    }
}

macro_rules! library_api {
    ($vis:vis struct $api:ident from $lib:literal {
        $($field_vis:vis $field:ident = $symbol:literal: fn($($arg:ty),*) $(-> $ret:ty)?;)*
    }) => {
        $vis struct $api {
            $($field_vis $field: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl $api {
            fn load() -> Option<Self> {
                let lib = $crate::symbols::Library::open($lib)?;
                Some(Self {
                    $($field: {
                        let symbol = lib.symbol($symbol)?;
                        unsafe { std::mem::transmute::<*mut std::ffi::c_void, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(symbol.as_ptr()) }
                    },)*
                })
            }
        }
    };
}

pub(crate) use library_api;
