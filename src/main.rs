use std::{process::Command, sync::Arc};

use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_pointer, wl_registry, wl_seat},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};

struct Seat {
    proxy: wl_seat::WlSeat,
}

impl Seat {
    fn new(globals: &wayland_client::globals::GlobalList, qh: &QueueHandle<Moxidle>) -> Self {
        let seat = globals
            .bind::<wl_seat::WlSeat, _, _>(qh, 1..=4, ())
            .unwrap();
        seat.get_pointer(qh, ());
        Self { proxy: seat }
    }
}

struct Moxidle {
    seat: Seat,
    notifier: ext_idle_notifier_v1::ExtIdleNotifierV1,
    idle_notification: ext_idle_notification_v1::ExtIdleNotificationV1,
}

impl Moxidle {
    fn new(globals: wayland_client::globals::GlobalList, qh: QueueHandle<Self>) -> Self {
        let notifier = globals
            .bind::<ext_idle_notifier_v1::ExtIdleNotifierV1, _, _>(&qh, 1..=1, ())
            .expect("Compositor doesn't support ext-idle-notifier-v1");

        let seat = Seat::new(&globals, &qh);

        let idle_notification = notifier.get_idle_notification(30 * 1000, &seat.proxy, &qh, ());

        Self {
            seat,
            notifier,
            idle_notification,
        }
    }

    fn lock_screen(&self) {
        run_command("hyprlock".into());
    }
}

fn run_command(command: Arc<str>) {
    let mut child = match Command::new("/bin/sh")
        .arg("-c")
        .arg(command.as_ref())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            log::error!("failed to execute command '{}': {}", command, err);
            return;
        }
    };

    std::thread::spawn(move || match child.wait() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            log::error!("command '{}' failed with exit status {}", command, status)
        }
        Err(err) => log::error!("failed to wait on command '{}': {}", command, err),
    });
}

impl Dispatch<wl_pointer::WlPointer, ()> for Moxidle {
    fn event(
        _: &mut Self,
        _: &wl_pointer::WlPointer,
        _: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for Moxidle {
    fn event(
        state: &mut Self,
        notification: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => {
                if &state.idle_notification == notification {
                    state.lock_screen();
                }
            }
            ext_idle_notification_v1::Event::Resumed => {
                // Handle resume if needed
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Moxidle {
    fn event(
        _state: &mut Self,
        _registry: &wl_registry::WlRegistry,
        _event: <wl_registry::WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _qh: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(Moxidle: ext_idle_notifier_v1::ExtIdleNotifierV1);
delegate_noop!(Moxidle: ignore wl_seat::WlSeat);

fn main() {
    env_logger::init();

    let conn = Connection::connect_to_env().expect("Wayland connection failed");
    let (globals, event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<Moxidle> = EventLoop::try_new().unwrap();
    let mut daemon = Moxidle::new(globals, qh.clone());

    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .unwrap();

    event_loop
        .run(None, &mut daemon, |_| {})
        .expect("Event loop failed");
}
