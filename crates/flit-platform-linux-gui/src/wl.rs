use std::ffi::c_void;
use std::io::Write;
use std::sync::Arc;
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

pub type Payload = Arc<crate::clipboard::Payload>;

#[derive(Default)]
pub struct State {
    input: Option<(WlSeat, u32)>,
    seats: Vec<(WlSeat, bool, bool)>,
    sources: Vec<WlDataSource>,
}

pub struct Wl {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    manager: Option<WlDataDeviceManager>,
    activation: Option<XdgActivationV1>,
    server_decorations: bool,
}

pub const DECORATION_MANAGER: &str = "zxdg_decoration_manager_v1";

pub fn offers_server_decorations<'a>(interfaces: impl IntoIterator<Item = &'a str>) -> bool {
    interfaces.into_iter().any(|name| name == DECORATION_MANAGER)
}

impl Wl {
    pub fn connect(display: *mut c_void) -> Option<Self> {
        if display.is_null() {
            return None;
        }
        let conn = Connection::from_backend(unsafe { Backend::from_foreign_display(display.cast()) });
        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let manager = globals.bind::<WlDataDeviceManager, _, _>(&qh, 2..=3, ()).ok();
        let activation = globals.bind::<XdgActivationV1, _, _>(&qh, 1..=1, ()).ok();
        let server_decorations = globals.contents().with_list(|list| offers_server_decorations(list.iter().map(|g| g.interface.as_str())));
        let mut state = State::default();
        let seats = globals.contents().with_list(|list| list.iter().filter(|g| g.interface == WlSeat::interface().name).map(|g| (g.name, g.version.min(5))).collect::<Vec<_>>());
        state.seats = seats.into_iter().map(|(name, version)| (globals.registry().bind::<WlSeat, _, _>(name, version, &qh, ()), false, false)).collect();
        queue.roundtrip(&mut state).ok()?;
        Some(Self { conn, queue, state, manager, activation, server_decorations })
    }

    pub fn server_decorations(&self) -> bool {
        self.server_decorations
    }

    pub fn pump(&mut self) {
        if let Err(e) = self.queue.dispatch_pending(&mut self.state) {
            tracing::debug!("wayland side queue: {e}");
        }
    }

    pub fn set_selection(&mut self, payload: Payload) -> bool {
        self.pump();
        let (Some(manager), Some((seat, serial))) = (&self.manager, &self.state.input) else { return false };
        let qh = self.queue.handle();
        let source = manager.create_data_source(&qh, payload.clone());
        for (mime, _) in payload.iter() {
            source.offer(mime.clone());
        }
        // Some compositors deliver drag offers only to the most recently bound device.
        // Bind briefly to publish the custom clipboard formats, then leave DnD to winit.
        let device = manager.get_data_device(seat, &qh, ());
        device.set_selection(Some(&source), *serial);
        // Send release without destroying the proxy until the server has delivered
        // any pending data_offer events, which create child objects on this queue.
        use wayland_sys::client::wayland_client_handle;
        // SAFETY: device is a live libwayland proxy bound at version >= 2.
        // The release request takes no arguments and is sent exactly once.
        unsafe {
            wayland_sys::ffi_dispatch!(
                wayland_client_handle(),
                wl_proxy_marshal_array,
                device.id().as_ptr().cast(),
                wl_data_device::REQ_RELEASE_OPCODE.into(),
                std::ptr::null_mut(),
            );
        }
        self.state.sources.push(source);
        let delivered = self.queue.roundtrip(&mut self.state).is_ok();
        let _ = self.conn.backend().destroy_object(&device.id());
        delivered
    }

    pub fn activate(&self, surface: *mut c_void, token: &str) -> bool {
        let Some(activation) = &self.activation else { return false };
        let Ok(id) = (unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }) else { return false };
        let Ok(surface) = WlSurface::from_id(&self.conn, id) else { return false };
        activation.activate(token.to_owned(), &surface);
        self.conn.flush().is_ok()
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event else { return };
        let Some((_, keyboard, pointer)) = state.seats.iter_mut().find(|(s, ..)| s == seat) else { return };
        if caps.contains(wl_seat::Capability::Keyboard) && !*keyboard {
            seat.get_keyboard(qh, seat.clone());
            *keyboard = true;
        }
        if caps.contains(wl_seat::Capability::Pointer) && !*pointer {
            seat.get_pointer(qh, seat.clone());
            *pointer = true;
        }
    }
}

impl Dispatch<WlKeyboard, WlSeat> for State {
    fn event(state: &mut Self, _: &WlKeyboard, event: wl_keyboard::Event, seat: &WlSeat, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Enter { serial, .. } | wl_keyboard::Event::Key { serial, .. } => state.input = Some((seat.clone(), serial)),
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, WlSeat> for State {
    fn event(state: &mut Self, _: &WlPointer, event: wl_pointer::Event, seat: &WlSeat, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_pointer::Event::Button { serial, .. } = event {
            state.input = Some((seat.clone(), serial));
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for State {
    fn event(_: &mut Self, _: &WlDataDeviceManager, _: <WlDataDeviceManager as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(_: &mut Self, _: &WlDataDevice, event: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_device::Event::Selection { id: Some(offer) } | wl_data_device::Event::Enter { id: Some(offer), .. } = event {
            offer.destroy();
        }
    }

    event_created_child!(State, WlDataDevice, [wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ())]);
}

impl Dispatch<WlDataOffer, ()> for State {
    fn event(_: &mut Self, _: &WlDataOffer, _: wl_data_offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlDataSource, Payload> for State {
    fn event(state: &mut Self, source: &WlDataSource, event: wl_data_source::Event, payload: &Payload, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                let Some(bytes) = payload.iter().find(|(t, _)| *t == mime_type).map(|(_, b)| b.clone()) else { return };
                let mut out = std::fs::File::from(fd);
                let _ = std::thread::Builder::new().name("flit-clipboard-send".into()).spawn(move || {
                    let _ = out.write_all(&bytes);
                });
            }
            wl_data_source::Event::Cancelled => {
                source.destroy();
                state.sources.retain(|s| s != source);
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgActivationV1, ()> for State {
    fn event(_: &mut Self, _: &XdgActivationV1, _: <XdgActivationV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

#[cfg(test)]
#[path = "../test/wl.rs"]
mod tests;
