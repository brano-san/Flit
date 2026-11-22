use super::*;

impl App {
    pub(super) fn subject(&self) -> Option<Subject> {
        let t = self.tab();
        let v = t.view.as_ref().filter(|v| v.snap().gen == t.gen)?;
        let l = v.listing();
        let in_bin = t.view_loc.is_recycle_bin();
        let dir_key = t.view_loc.to_windows().map(|p| p.as_bytes().to_vec()).unwrap_or_default();
        let base = Subject { title: t.title(), path: None, kind: Kind::Folder, entry: None, ext: Vec::new(), dir_key, many: None, drive: None, listing_gen: Some(t.gen) };
        match v.selected_count() {
            0 => Some(Subject { path: t.view_loc.to_windows().filter(|p| p.kind() != PathKind::Computer), ..base }),
            1 => {
                let i = v.focus.filter(|&f| v.is_selected(f)).or_else(|| v.selected().next())?;
                let e = l.entries[i as usize];
                let name = l.name(i);
                let row_dir = if crate::library::is_library(t.view_loc.path()) {
                    FsPath::parse_exact(crate::library::folder(l, i))
                } else if e.attrs & attr::NESTED != 0 {
                    t.view_loc.to_windows().map(|d| d.join(l.folder(i)))
                } else {
                    None
                };
                let path = if in_bin { recycle::recycled(l, i) } else { row_dir.as_ref().map_or_else(|| t.entry_location(i)?.to_windows(), |d| Some(d.join(name))) };
                let kind = if e.is_dir() { Kind::Folder } else { preview::kind_of(name) };
                let drive = (e.attrs & attr::VOLUME != 0).then(|| self.nav.sidebar.places.drives.iter().find(|d| d.path.as_bytes().get(..2).is_some_and(|p| p.eq_ignore_ascii_case(name)) && d.total > 0).map(|d| (d.total, d.free))).flatten();
                let dir_key = if in_bin { recycle::origin(l, i).to_vec() } else { row_dir.map_or(base.dir_key, |d| d.as_bytes().to_vec()) };
                Some(Subject { title: name::display(name).into_owned(), path, kind, entry: Some(e), ext: l.ext(i).to_vec(), dir_key, drive, listing_gen: None, ..base })
            }
            n => Some(Subject { title: crate::trn!(P::PreviewSelected, n), many: Some((n, v.selected_size())), path: None, ..base }),
        }
    }

    fn selection_types(&self) -> (Vec<(String, usize)>, usize, usize) {
        let t = self.tab();
        let Some(v) = t.view.as_ref() else { return (Vec::new(), 0, 0) };
        let l = v.listing();
        let mut counts: FxHashMap<Vec<u8>, usize> = FxHashMap::default();
        let (mut dirs, mut files) = (0, 0);
        for i in v.selected() {
            let dir = l.entries[i as usize].is_dir();
            if dir {
                dirs += 1;
            } else {
                files += 1;
            }
            let key = if dir { Vec::new() } else { l.ext(i).to_ascii_lowercase() };
            *counts.entry(key).or_default() += 1;
        }
        let mut out: Vec<(String, usize)> = counts.into_iter().map(|(ext, n)| (self.ext_type_label(&ext, ext.is_empty() && dirs > 0 && n <= dirs), n)).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        (out, dirs, files)
    }

    pub(super) fn ext_type_label(&self, ext: &[u8], dir: bool) -> String {
        if dir {
            return S::DetailsTypeFolder.t().into();
        }
        match self.type_names.get(ext) {
            Some(Some(n)) if !n.is_empty() => n.clone(),
            _ if ext.is_empty() => S::DetailsTypeFile.t().into(),
            _ => crate::tr!(S::DetailsTypeExtFile, ext = name::display(ext).to_uppercase()),
        }
    }

    pub(in crate::ui) fn expire_previous_preview(&mut self, now: Instant) {
        if let Some(previous) = &self.preview.previous {
            if previous.until <= now {
                self.preview.previous = None;
            } else {
                self.arm_timer(now, previous.until);
            }
        }
    }

    pub(super) fn pretty_available(&self) -> bool {
        matches!(self.preview.content, Content::Doc(_)) && self.preview.subject.as_ref().is_some_and(|s| preview::PrettyFormat::for_ext(&s.ext).is_some())
    }

    pub(super) fn toggle_pretty(&mut self) {
        let p = &mut self.preview;
        if matches!(p.pretty, Pretty::Loading) {
            return;
        }
        if let Pretty::On(raw) = std::mem::take(&mut p.pretty) {
            p.replace_text(raw);
            return;
        }
        let Some(s) = p.subject.as_ref().filter(|s| preview::PrettyFormat::for_ext(&s.ext).is_some()) else { return };
        let (Some(path), Some(entry)) = (s.path.clone(), s.entry) else { return };
        if !matches!(p.content, Content::Doc(_)) {
            return;
        }
        if entry.size > preview::PRETTY_FILE_LIMIT {
            self.status(S::PreviewPrettyTooLarge.t());
            return;
        }
        p.pretty = Pretty::Loading;
        self.effects.push(Effect::PrettyPreview(PreviewRequest { gen: p.gen, cancel: p.cancel.clone(), path, kind: s.kind, size: entry.size, details: false, px: 0 }));
    }

    pub fn on_pretty(&mut self, gen: u64, result: Result<Doc, preview::PrettyError>) {
        let p = &mut self.preview;
        if p.gen != gen || !matches!(p.pretty, Pretty::Loading) {
            return;
        }
        p.pretty = Pretty::Off;
        self.dirty = true;
        match result {
            Ok(doc) => {
                if let Content::Doc(raw) = std::mem::replace(&mut p.content, Content::Empty) {
                    p.pretty = Pretty::On(raw);
                    p.replace_text(doc);
                }
            }
            Err(preview::PrettyError::Invalid) => self.status(S::PreviewPrettyInvalid.t()),
            Err(preview::PrettyError::TooLarge) => self.status(S::PreviewPrettyTooLarge.t()),
            Err(preview::PrettyError::Unavailable) => self.status(S::PreviewPrettyUnavailable.t()),
        }
    }

    pub(in crate::ui) fn sync_preview(&mut self, now: Instant) {
        let shown = self.preview.state.visible && !self.search.active && !self.on_home();
        let subject = if shown { self.subject() } else { None };
        let key = subject.as_ref().map_or(0, Subject::key);
        let wants_props = shown && self.preview.look.card && self.preview.state.details;
        let props_key = if wants_props { key } else { 0 };
        if props_key != self.preview.props_key {
            self.preview.props_key = props_key;
            self.preview.props.clear();
            self.preview.props_loading = false;
            let path = subject.as_ref().filter(|s| s.many.is_none() && s.entry.is_some()).and_then(|s| s.path.clone());
            if let (true, Some(path), Some(s)) = (wants_props, path, subject.as_ref()) {
                let gen = self.gen();
                let p = &mut self.preview;
                p.props_gen = gen;
                p.props_loading = true;
                let cancel = GenToken::default();
                cancel.set(gen);
                let req = PreviewRequest { gen, cancel, path, kind: s.kind, size: 0, details: true, px: 0 };
                self.effects.push(Effect::Preview(req));
            }
        }
        if key == self.preview.key {
            return;
        }
        let gen = self.gen();
        let archived = self.archive_view().is_some();
        let (types, dirs, files) = if subject.as_ref().is_some_and(|s| s.many.is_some()) { self.selection_types() } else { (Vec::new(), 0, 0) };
        let p = &mut self.preview;
        let mut previous = p.previous.take().filter(|v| v.until > now);
        let cached = if previous.as_ref().is_some_and(|v| v.key == key) { previous.take() } else { None };
        let had_image = p.image.is_some();
        if p.key != 0 && p.previous_size().is_some_and(|n| n <= PREVIOUS_PREVIEW_BYTES) {
            if let Content::Pdf(pdf) = &mut p.content {
                pdf.cancel();
            }
            previous = Some(PreviousPreview {
                key: p.key,
                until: now + KEEP_PREVIEW_FOR,
                content: std::mem::take(&mut p.content),
                image: p.image.take(),
                px: p.image_px,
                pretty: match std::mem::take(&mut p.pretty) {
                    ready @ Pretty::On(_) => ready,
                    _ => Pretty::Off,
                },
            });
        }
        p.previous = previous;
        p.pretty = Pretty::Off;
        p.key = key;
        p.gen = gen;
        p.cancel.set(gen);
        (p.scroll, p.hscroll) = (0.0, 0.0);
        p.doc = None;
        p.pick = Pick::default();
        p.media = Media::default();
        p.types = types;
        if dirs + files > 0 {
            if let Some(s) = p.subject.as_mut() {
                s.title = String::new();
            }
        }
        p.image = None;
        if had_image {
            p.image_rev += 1;
        }
        if std::mem::take(&mut p.host).gen != 0 {
            self.effects.push(Effect::Host(HostCmd::Close));
        }
        self.expire_previous_preview(now);
        let p = &mut self.preview;
        p.content = Content::Empty;
        p.requested = None;
        p.subject = subject.clone().map(|mut s| {
            if s.many.is_some() {
                s.title = format!("{}, {}", crate::trn!(P::PreviewFolders, dirs), crate::trn!(P::PreviewFiles, files));
            }
            s
        });
        let Some(s) = subject.filter(|s| s.many.is_none() && s.drive.is_none()) else { return };
        let Some(path) = s.path else { return };
        if archived {
            p.content = Content::Basic(Some(Note::Archived));
            return;
        }
        if s.entry.is_some_and(|e| preview::waits_for_download(e.attrs)) && p.download != key {
            p.content = Content::Basic(Some(Note::OnlineOnly));
            return;
        }
        let side = p.r.body.w.max(p.r.body.h) * 2.0;
        let px = (side as u32).clamp(256, IMAGE_PX);
        if let Some(cached) = cached.filter(|cached| cached.px >= px) {
            p.content = cached.content;
            p.image = cached.image;
            p.image_px = cached.px;
            p.pretty = cached.pretty;
            if p.image.is_some() {
                p.image_rev += 1;
            }
            return;
        }
        p.image_px = px;
        p.content = Content::Loading;
        p.requested = Some(now);
        let req = PreviewRequest { gen, cancel: p.cancel.clone(), path, kind: s.kind, size: s.entry.map_or(0, |e| e.size), details: false, px };
        self.effects.push(Effect::Preview(req));
    }

    pub fn on_preview(&mut self, gen: u64, data: PreviewData) {
        if gen != 0 && gen == self.preview.copy_gen {
            self.preview.copy_gen = 0;
            let full = match data {
                PreviewData::Image(b) => Some(b),
                _ => self.preview.image.clone(),
            };
            self.effects.extend(full.map(Effect::SetClipboardImage));
            return;
        }
        if let PreviewData::Props(groups) = &data {
            if self.quick_props(gen, groups) || self.properties_props(gen, groups) {
                return;
            }
            if gen == self.preview.props_gen && gen != 0 {
                self.preview.props.clone_from(groups);
                self.preview.props_loading = false;
                self.dirty = true;
                return;
            }
        }
        let p = &mut self.preview;
        if gen != p.gen {
            return;
        }
        self.dirty = true;
        let (content, bitmap, hosted) = match data {
            PreviewData::Doc(d) => (Content::Doc(d), None, None),
            PreviewData::Image(b) => (Content::Image, Some(b), None),
            PreviewData::Folder(f) => (Content::Folder(f), None, None),
            PreviewData::Media { video, poster } => (Content::Media { video }, poster, Some(Hosted::Media { video })),
            PreviewData::Handler(clsid) => (Content::Handler, None, Some(Hosted::Handler(clsid))),
            PreviewData::Basic { poster, note } => (Content::Basic(note), poster, None),
            PreviewData::Props(_) => (Content::Basic(None), None, None),
            PreviewData::Pdf(sizes) => (Content::Pdf(pdf_view::PdfView::new(sizes)), None, None),
        };
        p.content = content;
        p.pick = Pick::default();
        if let Some(b) = bitmap {
            p.image = Some(b);
            p.image_rev += 1;
        }
        let (Some(what), Some(path)) = (hosted, p.subject.as_ref().and_then(|s| s.path.clone())) else { return };
        p.host = Host { gen, handler: matches!(what, Hosted::Handler(_)), video: what == Hosted::Media { video: true }, ..Host::default() };
        let (volume, bg) = (p.state.volume, self.theme().bg.0);
        self.effects.push(Effect::Host(HostCmd::Show { gen, what, path, rect: None, volume, bg }));
    }

    pub(in crate::ui) fn on_hosted(&mut self, ev: HostEvent) {
        let p = &mut self.preview;
        match ev {
            HostEvent::Ready { gen, ms } if gen == p.host.gen => {
                let total = p.requested.take().map_or(0.0, |t| t.elapsed().as_secs_f64() * 1e3);
                tracing::debug!("hosted preview shown {total:.1} ms after selection, {ms} ms in the handler");
            }
            HostEvent::Failed { gen, message } if gen == p.host.gen => {
                let media = !p.host.handler;
                p.host = Host::default();
                p.content = if media { Content::Basic(Some(Note::CannotPlay(message))) } else { Content::Basic(Some(Note::Message(message))) };
            }
            HostEvent::Media { gen, position, duration, playing } if gen == p.host.gen => p.media = Media { position, duration, playing },
            HostEvent::Frame(frames) => match frames.take() {
                Some((gen, frame)) if gen == p.host.gen && p.host.video => {
                    p.image = Some(frame);
                    p.image_rev += 1;
                }
                _ => return,
            },
            HostEvent::Released { gen } if gen == p.host.gen => {
                p.host = Host::default();
                p.content = Content::Basic(None);
                p.released = true;
                self.effects.push(Effect::Host(HostCmd::Close));
            }
            _ => return,
        }
        self.dirty = true;
    }

    pub(in crate::ui) fn reload_released_preview(&mut self) {
        if std::mem::take(&mut self.preview.released) {
            self.preview.key = 0;
            self.dirty = true;
        }
    }

    pub(super) fn place_host(&mut self) {
        let p = &self.preview;
        if p.host.gen == 0 {
            return;
        }
        let covered = self.nav.palette.is_some() || self.ops.jobs.open || self.dialogs.modal() || self.search.active || p.look.card;
        let want = match () {
            _ if covered || p.r.pane.w <= 0.0 => None,
            _ if p.host.handler => Some(p.r.body),
            _ if p.host.video && p.host.started && p.r.media.h > 0.0 => Some(p.r.media),
            _ => None,
        };
        if want != p.host.placed {
            self.preview.host.placed = want;
            self.effects.push(Effect::Host(HostCmd::Place(want)));
        }
    }
}
