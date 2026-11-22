use flit_core::FsPath;
use flit_platform::parse_process::{ARCHIVE_HEADER, EXIT_PASSWORD_REQUIRED, EXIT_TOO_LARGE, PROBE_TASK};
use std::ffi::OsString;
use std::io::Write;

pub(crate) fn serve(mut args: impl Iterator<Item = OsString>) -> i32 {
    let Some(task) = args.next() else { return 2 };
    let Some(input) = args.next() else { return 2 };
    let out = std::io::stdout();
    let mut out = out.lock();
    let result = match task.to_str() {
        Some(PROBE_TASK) if args.next().is_none() => Ok(()),
        Some("archive") if args.next().is_none() => {
            use flit_archive::PreviewError;
            let Some(path) = FsPath::parse_bytes(&crate::wtf8(&input)) else { return 2 };
            flit_archive::preview_paths(&path)
                .map_err(|error| match error {
                    PreviewError::TooLarge => EXIT_TOO_LARGE,
                    PreviewError::PasswordRequired => EXIT_PASSWORD_REQUIRED,
                    PreviewError::Failed => 1,
                })
                .and_then(|text| out.write_all(ARCHIVE_HEADER).and_then(|()| out.write_all(text.as_bytes())).map_err(|_| 1))
        }
        #[cfg(target_os = "linux")]
        Some("webp") => {
            let size = args.next().and_then(|arg| arg.to_str()?.parse::<u32>().ok());
            let Some(size) = size.filter(|&size| size != 0 && args.next().is_none()) else { return 2 };
            flit_platform_linux_gui::decode_webp(std::path::Path::new(&input), size, &mut out).map_err(|error| match error.kind() {
                std::io::ErrorKind::InvalidData => flit_platform::parse_process::EXIT_UNSUPPORTED,
                _ => 1,
            })
        }
        _ => return 2,
    };
    result.map_or_else(|code| code, |()| i32::from(out.flush().is_err()))
}
