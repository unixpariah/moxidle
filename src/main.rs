//mod screensaver;
mod config;

use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use config::{FullConfig, MoxidleConfig, TimeoutConfig};
use std::{error, ops::Deref, process::Command, sync::Arc};
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_pointer, wl_registry, wl_seat},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};

struct TimeoutCmd {
    config: TimeoutConfig,
    notification: ext_idle_notification_v1::ExtIdleNotificationV1,
}

impl Deref for TimeoutCmd {
    type Target = TimeoutConfig;

    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

impl TimeoutCmd {
    fn new(
        config: TimeoutConfig,
        qh: &QueueHandle<Moxidle>,
        seat: &wl_seat::WlSeat,
        notifier: &ext_idle_notifier_v1::ExtIdleNotifierV1,
    ) -> Self {
        let notification = notifier.get_idle_notification(config.timeout(), seat, qh, ());

        Self {
            config,
            notification,
        }
    }
}

struct Moxidle {
    seat: wl_seat::WlSeat,
    notifier: ext_idle_notifier_v1::ExtIdleNotifierV1,
    timeout_cmds: Vec<TimeoutCmd>,
    config: MoxidleConfig,
}

impl Moxidle {
    fn new(
        globals: wayland_client::globals::GlobalList,
        qh: QueueHandle<Self>,
    ) -> Result<Self, Box<dyn error::Error>> {
        let notifier = globals
            .bind::<ext_idle_notifier_v1::ExtIdleNotifierV1, _, _>(&qh, 1..=1, ())
            .expect("Compositor doesn't support ext-idle-notifier-v1");

        let seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=4, ())?;
        seat.get_pointer(&qh, ());

        let (config, timeouts) = FullConfig::load()?.apply();

        let timeout_cmds = timeouts
            .into_iter()
            .map(|timeout| TimeoutCmd::new(timeout, &qh, &seat, &notifier))
            .collect();

        Ok(Self {
            timeout_cmds,
            config,
            notifier,
            seat,
        })
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
        let Some(timeout_cmd) = state
            .timeout_cmds
            .iter()
            .find(|timeout_cmd| timeout_cmd.notification == *notification)
        else {
            return;
        };

        match event {
            ext_idle_notification_v1::Event::Idled => {
                if let Some(on_timeout) = timeout_cmd.on_timeout.as_ref() {
                    run_command(Arc::clone(on_timeout));
                }
            }
            ext_idle_notification_v1::Event::Resumed => {
                if let Some(on_resume) = timeout_cmd.on_resume.as_ref() {
                    run_command(Arc::clone(on_resume));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Moxidle {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: <wl_registry::WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(Moxidle: ext_idle_notifier_v1::ExtIdleNotifierV1);
delegate_noop!(Moxidle: ignore wl_seat::WlSeat);

fn main() -> Result<(), Box<dyn error::Error>> {
    env_logger::init();

    let conn = Connection::connect_to_env().expect("Wayland connection failed");
    let (globals, event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<Moxidle> = EventLoop::try_new().unwrap();
    let mut moxidle = Moxidle::new(globals, qh.clone())?;

    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .unwrap();

    event_loop
        .run(None, &mut moxidle, |_| {})
        .expect("Event loop failed");

    Ok(())
}
