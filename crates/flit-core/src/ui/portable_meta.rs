use super::archive::Kind;
use super::form::{Control, Form};
use super::{App, Effect};
use crate::i18n::{self, S};
use crate::location::ProviderId;
use crate::path::{FsPath, PathKind};
use crate::portable_meta::{Metadata, Request, xmp_path};

impl App {
    pub(super) fn file_metadata(&mut self) {
        if !self.metadata_enabled() {
            return self.status(S::PortableMetaDisabled.t());
        }
        let tab = self.tab();
        let Some(view) = &tab.view else { return self.status(S::PortableMetaSelect.t()); };
        let Ok([i]): Result<[u32; 1], _> = self.selected_items().try_into() else { return self.status(S::PortableMetaSelect.t()); };
        if view.listing().entries[i as usize].is_dir() || tab.view_loc.provider() != ProviderId::Local || tab.view_loc.is_recycle_bin() || self.archive_view().is_some() {
            return self.status(S::PortableMetaSelect.t());
        }
        let Some(path) = tab.entry_location(i).and_then(|item| item.to_windows()).filter(|p| matches!(p.kind(), PathKind::Drive | PathKind::Unix)) else {
            return self.status(S::PortableMetaSelect.t());
        };
        let Some(sidecar) = xmp_path(&path) else { return self.status(S::PortableMetaSelect.t()); };
        self.queue_metadata(Request::Read(path), sidecar);
    }

    fn queue_metadata(&mut self, req: Request, sidecar: FsPath) {
        self.metadata_request_id += 1;
        let id = self.metadata_request_id;
        let wait_for = match &req {
            Request::Read(_) => S::PortableMetaLoading,
            Request::Store { .. } | Request::Import { .. } | Request::Export { .. } => S::PortableMetaSaving,
        };
        let path = req.path().clone();
        self.effects.push(Effect::PortableMetadata { id, req });
        self.open_form(Form::new(
            Kind::PortableMetaWait { id, path, sidecar },
            S::PortableMetaTitle.t(),
            vec![Control::paragraph(wait_for.t().to_owned())],
            vec![S::ArchiveClose.t()],
        ));
    }

    pub fn on_portable_metadata(&mut self, id: u64, result: Result<Metadata, String>) {
        let Some(f) = self.jobs.form.take() else { return; };
        let Kind::PortableMetaWait { id: expected, path, sidecar } = f.kind else {
            self.jobs.form = Some(f);
            return;
        };
        if id != expected {
            self.jobs.form = Some(Form { kind: Kind::PortableMetaWait { id: expected, path, sidecar }, ..f });
            return;
        }
        let value = match result {
            Ok(value) => value,
            Err(e) => {
                self.status(crate::tr!(S::PortableMetaFailed, error = i18n::localize(&e)));
                self.layout();
                self.dirty = true;
                return;
            }
        };
        if let Some(parent) = path.parent() {
            for tab in self.tabs.iter_mut().filter(|t| t.view_loc.to_windows().is_some_and(|dir| dir.same(&parent))) {
                tab.props.forget(path.file_name(), None);
            }
        }
        let rating = (0..=10).map(|r| {
            if r == 0 { S::PortableMetaUnrated.t().to_owned() } else { Metadata { rating: r, ..Metadata::default() }.display_rating() }
        }).collect();
        let controls = vec![
            Control::line(&path.display()),
            Control::choice(S::PortableMetaRating.t(), rating, usize::from(value.rating)),
            Control::text(S::PortableMetaComment.t(), &value.comment, ""),
            Control::text(S::PortableMetaTags.t(), &value.tags.join(", "), S::PortableMetaTagHint.t()),
            Control::text(S::PortableMetaXmpPath.t(), &sidecar.display(), S::ArchiveExtractToHint.t()),
        ];
        self.open_form(Form::new(
            Kind::PortableMeta { path, sidecar, value },
            S::PortableMetaTitle.t(),
            controls,
            vec![S::PortableMetaSave.t(), S::PortableMetaImport.t(), S::PortableMetaExport.t(), S::ArchiveClose.t()],
        ));
    }

    pub(super) fn submit_portable_metadata(&mut self, mut form: Form, button: usize) -> Option<Form> {
        let Kind::PortableMeta { path, sidecar: original_sidecar, value } = &form.kind else { return None; };
        if !self.metadata_enabled() {
            self.status(S::PortableMetaDisabled.t());
            return None;
        }
        let after = Metadata { rating: form.choice(1) as u8, comment: form.text(2).to_owned(), tags: Metadata::tags_from_text(form.text(3)) };
        if button != 1 && !after.valid() {
            form.say(S::PortableMetaInvalid.t(), true);
            return Some(form);
        }
        let sidecar = if button == 0 || form.text(4) == original_sidecar.display().as_ref() {
            Some(original_sidecar.clone())
        } else {
            self.caps.native_path(form.text(4).trim()).filter(|side| side.file_name().to_ascii_lowercase().ends_with(b".xmp") && !side.same(path))
        };
        let Some(sidecar) = sidecar else {
            form.say(S::PortableMetaInvalidXmp.t(), true);
            return Some(form);
        };
        let req = match button {
            0 => Request::Store { path: path.clone(), before: value.clone(), after },
            1 => Request::Import { path: path.clone(), sidecar: sidecar.clone(), before: value.clone() },
            _ => Request::Export { path: path.clone(), sidecar: sidecar.clone(), value: after },
        };
        self.queue_metadata(req, sidecar);
        None
    }

    pub fn metadata_enabled(&self) -> bool {
        self.caps.unix_paths || self.metadata_ads
    }
}

impl App {
    pub(super) fn set_metadata_ads(&mut self, on: bool) {
        if self.metadata_ads == on { return; }
        self.metadata_ads = on;
        for tab in &mut self.tabs {
            tab.props.invalidate();
        }
        self.dirty = true;
    }
}
