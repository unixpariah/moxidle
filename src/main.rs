use wayland_client::{
    protocol::{wl_registry, wl_seat},
    Connection, Dispatch,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};

#[derive(Default)]
struct Seat {
    proxy: Option<wl_seat::WlSeat>,
    capabilities: Option<wayland_client::WEnum<wl_seat::Capability>>,
    name: String,
}

#[derive(Default)]
struct Moxidle {
    exit: bool,
    seat: Seat,
    notifier: Option<ext_idle_notifier_v1::ExtIdleNotifierV1>,
    notification: Option<ext_idle_notification_v1::ExtIdleNotificationV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Moxidle {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: <wl_registry::WlRegistry as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_seat" => {
                    state.seat.proxy =
                        Some(registry.bind::<wl_seat::WlSeat, _, _>(name, version, qh, ()))
                }
                "ext_idle_notifier_v1" => {
                    state.notifier = Some(
                        registry.bind::<ext_idle_notifier_v1::ExtIdleNotifierV1, _, _>(
                            name,
                            version,
                            qh,
                            (),
                        ),
                    )
                }
                "ext_idle_notification_v1" => {
                    state.notification = Some(
                        registry.bind::<ext_idle_notification_v1::ExtIdleNotificationV1, _, _>(
                            name,
                            version,
                            qh,
                            (),
                        ),
                    )
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {}
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Moxidle {
    fn event(
        state: &mut Self,
        proxy: &wl_seat::WlSeat,
        event: <wl_seat::WlSeat as wayland_client::Proxy>::Event,
        data: &(),
        conn: &Connection,
        qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            wl_seat::Event::Name { name } => {
                state.seat.name = name;
            }
            wl_seat::Event::Capabilities { capabilities } => {
                state.seat.capabilities = Some(capabilities)
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_idle_notifier_v1::ExtIdleNotifierV1, ()> for Moxidle {
    fn event(
        state: &mut Self,
        proxy: &ext_idle_notifier_v1::ExtIdleNotifierV1,
        event: <ext_idle_notifier_v1::ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        data: &(),
        conn: &Connection,
        qhandle: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for Moxidle {
    fn event(
        state: &mut Self,
        proxy: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: <ext_idle_notification_v1::ExtIdleNotificationV1 as wayland_client::Proxy>::Event,
        data: &(),
        conn: &Connection,
        qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => {}
            ext_idle_notification_v1::Event::Resumed => {}
            _ => {}
        };
    }
}

fn main() {
    env_logger::init();

    let conn = Connection::connect_to_env().unwrap();
    let display = conn.display();

    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut moxidle = Moxidle::default();

    _ = display.get_registry(&qh, ());

    event_queue.dispatch_pending(&mut moxidle).unwrap();
    event_queue.roundtrip(&mut moxidle).unwrap();

    while !moxidle.exit {
        event_queue.dispatch_pending(&mut moxidle).unwrap();
    }
}
