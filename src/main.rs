mod config;
mod screensaver;

use calloop::{channel, generic::Generic, EventLoop, Interest, Mode};
use calloop_wayland_source::WaylandSource;
use config::{FullConfig, MoxidleConfig, TimeoutConfig};
use inotify::{Inotify, WatchMask};
use std::{error::Error, ops::Deref, os::fd::AsRawFd, process::Command, sync::Arc};
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalList, GlobalListContents},
    protocol::{wl_pointer, wl_registry, wl_seat},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct TimeoutHandler {
    config: TimeoutConfig,
    notification: ext_idle_notification_v1::ExtIdleNotificationV1,
}

impl TimeoutHandler {
    fn new(
        config: TimeoutConfig,
        qh: &QueueHandle<Moxidle>,
        seat: &wl_seat::WlSeat,
        notifier: &ext_idle_notifier_v1::ExtIdleNotifierV1,
    ) -> Self {
        let notification = notifier.get_idle_notification(config.timeout_millis(), seat, qh, ());

        Self {
            config,
            notification,
        }
    }

    fn on_timeout(&self) -> Option<&Arc<str>> {
        self.config.on_timeout.as_ref()
    }

    fn on_resume(&self) -> Option<&Arc<str>> {
        self.config.on_resume.as_ref()
    }
}

struct Moxidle {
    seat: wl_seat::WlSeat,
    notifier: ext_idle_notifier_v1::ExtIdleNotifierV1,
    timeouts: Vec<TimeoutHandler>,
    config: MoxidleConfig,
    inhibited: bool,
    qh: QueueHandle<Self>,
    inotify: Inotify,
}

impl Deref for Moxidle {
    type Target = MoxidleConfig;

    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

impl Moxidle {
    fn init(globals: GlobalList, qh: QueueHandle<Self>) -> Result<Self> {
        let notifier = globals
            .bind(&qh, 1..=1, ())
            .expect("Compositor doesn't support ext-idle-notifier-v1");

        let seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=4, ())?;
        seat.get_pointer(&qh, ());

        let (config, config_path) = FullConfig::load()?;
        let (general_config, timeout_configs) = config.split_into_parts();

        let timeouts = timeout_configs
            .into_iter()
            .map(|cfg| TimeoutHandler::new(cfg, &qh, &seat, &notifier))
            .collect();

        let inotify = Inotify::init()?;
        inotify.watches().add(
            config_path.parent().unwrap(),
            WatchMask::MODIFY | WatchMask::CREATE | WatchMask::DELETE,
        )?;

        Ok(Self {
            inotify,
            timeouts,
            config: general_config,
            notifier,
            seat,
            inhibited: false,
            qh,
        })
    }

    fn handle_app_event(&mut self, event: Event) {
        println!("{:?}", event);
        match event {
            Event::ScreensaverInhibit(inhibited) => {
                self.inhibited = inhibited;
                if !inhibited {
                    self.reset_idle_timers();
                }
            }
            Event::SessionLocked(locked) => match locked {
                true => {
                    if let Some(lock_cmd) = self.config.lock_cmd.as_ref() {
                        execute_command(lock_cmd.clone())
                    }
                }
                false => {
                    if let Some(unlock_cmd) = self.config.unlock_cmd.as_ref() {
                        execute_command(unlock_cmd.clone())
                    }
                }
            },
            Event::BlockInhibited(ihibition) => {}
        }
    }

    fn reset_idle_timers(&mut self) {
        self.timeouts.iter_mut().for_each(|handler| {
            handler.notification = self.notifier.get_idle_notification(
                handler.config.timeout_millis(),
                &self.seat,
                &self.qh,
                (),
            );
        });
    }

    fn reload_config(&mut self) -> Result<()> {
        let (new_config, _) = FullConfig::load()?;
        let (general_config, timeout_configs) = new_config.split_into_parts();

        self.config = general_config;
        self.timeouts = timeout_configs
            .into_iter()
            .map(|cfg| TimeoutHandler::new(cfg, &self.qh, &self.seat, &self.notifier))
            .collect();

        log::info!("Configuration reloaded successfully");
        Ok(())
    }
}

#[derive(Debug)]
enum Event {
    ScreensaverInhibit(bool),
    SessionLocked(bool),
    BlockInhibited(String),
}

fn execute_command(command: Arc<str>) {
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

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for Moxidle {
    fn event(
        state: &mut Self,
        notification: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if state.inhibited {
            log::debug!("Ignoring idle event due to active inhibition");
            return;
        }

        let Some(handler) = state
            .timeouts
            .iter()
            .find(|h| h.notification == *notification)
        else {
            return;
        };

        match event {
            ext_idle_notification_v1::Event::Idled => {
                if let Some(cmd) = handler.on_timeout() {
                    log::info!("Executing timeout command: {}", cmd);
                    execute_command(cmd.clone());
                }
            }
            ext_idle_notification_v1::Event::Resumed => {
                if let Some(cmd) = handler.on_resume() {
                    log::info!("Executing resume command: {}", cmd);
                    execute_command(cmd.clone());
                }
            }
            _ => (),
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Moxidle {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(Moxidle: wl_pointer::WlPointer);
delegate_noop!(Moxidle: ext_idle_notifier_v1::ExtIdleNotifierV1);
delegate_noop!(Moxidle: ignore wl_seat::WlSeat);

//async fn receive_battery_task(sender: EventSender) -> zbus::Result<()> {
//    let connection = zbus::Connection::system().await?;
//    let upower = UPowerProxy::new(&connection).await?;
//    let mut stream = upower.receive_on_battery_changed().await;
//    while let Some(event) = stream.next().await {
//        let _ = sender.send(Event::OnBattery(event.get().await?));
//    }
//    Ok(())
//}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let conn = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let mut event_loop = EventLoop::try_new()?;
    let mut moxidle = Moxidle::init(globals, qh.clone())?;

    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|e| format!("Failed to insert Wayland source: {}", e))?;

    let inotify_source = Generic::new(
        unsafe { calloop::generic::FdWrapper::new(moxidle.inotify.as_raw_fd()) },
        Interest::READ,
        Mode::Level,
    );
    event_loop
        .handle()
        .insert_source(inotify_source, |_, _, state| {
            let mut buffer = [0; 4096];
            if let Err(e) = state.inotify.read_events(&mut buffer) {
                log::error!("Inotify read error: {}", e);
                return Ok(calloop::PostAction::Continue);
            }

            if let Err(e) = state.reload_config() {
                log::error!("Config reload failed: {}", e);
            }
            Ok(calloop::PostAction::Continue)
        })?;

    let (executor, scheduler) = calloop::futures::executor()?;
    let (event_sender, event_receiver) = channel::channel();

    scheduler.schedule(async move {
        if let Err(e) = screensaver::serve(event_sender).await {
            log::error!("D-Bus error: {}", e);
        }
    })?;

    event_loop.handle().insert_source(executor, |_, _, _| ())?;
    event_loop
        .handle()
        .insert_source(event_receiver, |event, _, state| {
            if let channel::Event::Msg(event) = event {
                state.handle_app_event(event);
            }
        })?;

    event_loop.run(None, &mut moxidle, |_| {})?;
    Ok(())
}
