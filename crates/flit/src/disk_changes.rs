use flit_core::disk_changes::{Error, Reply, Request};
use flit_platform::std_path;
use std::fs::{self, File};
use std::io::Write;

pub fn run(req: Request) -> Reply {
    match req {
        Request::Read { root, filter, elevated, ctl } => {
            #[cfg(windows)]
            let report = if elevated {
                flit_platform_windows::disk_changes::read_elevated(&root, &filter, &|| ctl.is_cancelled())
            } else {
                flit_platform_windows::disk_changes::read(&root, &filter, &|| ctl.is_cancelled())
            };
            #[cfg(not(windows))]
            let report = {
                let _ = (root, filter, elevated, ctl);
                Err(Error::Failed(flit_core::i18n::S::DiskChangesLocalOnly.en().to_owned()))
            };
            Reply::Read(report)
        }
        Request::Export { path, report, ctl } => {
            let write = || {
                let target = std_path(&path);
                let text = report.csv();
                if ctl.is_cancelled() { return Err(Error::Cancelled); }
                let mut file = File::create_new(&target).map_err(error)?;
                let result = file.write_all(text.as_bytes()).and_then(|()| file.sync_data()).map_err(error);
                if result.is_err() {
                    drop(file);
                    let _ = fs::remove_file(&target);
                }
                result
            };
            Reply::Exported(write().map(|()| path))
        }
    }
}

fn error(e: std::io::Error) -> Error {
    Error::Failed(e.to_string())
}
