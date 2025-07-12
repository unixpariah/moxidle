#[cfg(feature = "audio")]
mod audio;
mod config;
mod login;
mod screensaver;
mod upower;
mod usb;

use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use clap::Parser;
use config::Condition;
use config::{Config, ListenerConfig, MoxidleConfig};
use env_logger::Builder;
use log::LevelFilter;
use rusb::UsbContext;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::{ops::Deref, path::PathBuf, sync::Arc, time::Instant};
use tokio::sync::oneshot;
use upower::{BatteryLevel, BatteryState, LevelComparison, Power, PowerSource};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalList, GlobalListContents, registry_queue_init},
    protocol::{wl_pointer, wl_registry, wl_seat},
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};

struct TimeoutHandler {
    config: ListenerConfig,
    notification: Option<ext_idle_notification_v1::ExtIdleNotificationV1>,
}

impl TimeoutHandler {
    fn new(config: ListenerConfig) -> Self {
        Self {
            config,
            notification: None,
        }
    }

    fn on_timeout(&self) -> Option<&Arc<str>> {
        self.config.on_timeout.as_ref()
    }

    fn on_resume(&self) -> Option<&Arc<str>> {
        self.config.on_resume.as_ref()
    }
}

#[derive(Default)]
struct Inhibitors {
    #[cfg(feature = "audio")]
    audio_inhibitor: bool,
    dbus_inhibitor: bool,
    systemd_inhibitor: bool,
}

impl Inhibitors {
    fn active(&self) -> bool {
        let mut active = self.dbus_inhibitor || self.systemd_inhibitor;
        #[cfg(feature = "audio")]
        {
            active |= self.audio_inhibitor;
        }
        active
    }
}

#[derive(PartialEq, Copy, Clone)]
enum LockState {
    Locked,
    Unlocked,
}

struct State {
    // Notification used to detect when system should resume after being locked externally.
    notification: Option<ext_idle_notification_v1::ExtIdleNotificationV1>,
    lock_state: LockState,
    active_since: Option<Instant>,
    emit_sender: mpsc::Sender<()>,
}

impl State {
    fn new(emit_sender: mpsc::Sender<()>) -> Self {
        Self {
            notification: None,
            active_since: None,
            lock_state: LockState::Unlocked,
            emit_sender,
        }
    }

    fn set_lock_state(&mut self, lock_state: LockState) {
        if self.lock_state != lock_state {
            if let Err(e) = self.emit_sender.send(()) {
                log::error!("Failed to send emit event: {e}");
            }
            self.lock_state = lock_state;
            if self.lock_state == LockState::Locked {
                self.active_since = Some(Instant::now());
            }
        }
    }
}

struct Moxidle {
    state: State,
    seat: wl_seat::WlSeat,
    notifier: ext_idle_notifier_v1::ExtIdleNotifierV1,
    listeners: Vec<TimeoutHandler>,
    config: MoxidleConfig,
    inhibitors: Inhibitors,
    qh: QueueHandle<Self>,
    power: Power,
    usb_context: Option<rusb::Context>,
}

impl Deref for Moxidle {
    type Target = MoxidleConfig;

    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

impl Moxidle {
    fn new(
        globals: GlobalList,
        qh: QueueHandle<Self>,
        config_path: Option<PathBuf>,
        emit_sender: mpsc::Sender<()>,
        usb_context: Option<rusb::Context>,
    ) -> anyhow::Result<Self> {
        let notifier = globals
            .bind(&qh, 1..=1, ())
            .expect("Compositor doesn't support ext-idle-notifier-v1");

        let seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=4, ())?;
        seat.get_pointer(&qh, ());

        let (general_config, listener_configs) = Config::load(config_path)?;

        let listeners = listener_configs
            .into_iter()
            .map(TimeoutHandler::new)
            .collect();

        Ok(Self {
            usb_context,
            state: State::new(emit_sender),
            power: Power::default(),
            listeners,
            config: general_config,
            notifier,
            seat,
            inhibitors: Inhibitors::default(),
            qh,
        })
    }

    fn should_ignore<F>(&self, condition_predicate: F) -> bool
    where
        F: Fn(&Condition) -> bool,
    {
        !self
            .listeners
            .iter()
            .any(|timeout| timeout.config.conditions.iter().any(&condition_predicate))
    }

    fn handle_app_event(&mut self, event: Event) {
        match event {
            Event::GetLockState(sender) => {
                if sender.send(self.state.lock_state).is_err() {
                    log::error!("Failed to send lock state");
                }
            }
            Event::GetActiveTime(sender) => {
                if let Some(time) = self.state.active_since {
                    if sender.send(time.elapsed().as_secs() as u32).is_err() {
                        log::error!("Failed to send lock active time");
                    }
                } else if sender.send(0).is_err() {
                    log::error!("Failed to send lock active time");
                }
            }
            Event::BatteryState(state) => {
                self.power.update_state(state);
                self.reset_idle_timers();
            }
            Event::BatteryLevel(level) => {
                self.power.update_level(level);
                self.reset_idle_timers();
            }
            Event::OnBattery(on_battery) => {
                self.power.update_source(on_battery);
                self.reset_idle_timers();
            }
            Event::BatteryPercentage(battery) => {
                self.power.update_percentage(battery);
                self.reset_idle_timers();
            }
            Event::SimulateUserActivity | Event::Usb => {
                self.reset_idle_timers();
            }
            Event::ScreenSaverInhibit(inhibited) => {
                self.inhibitors.dbus_inhibitor = inhibited;
                self.reset_idle_timers();
            }
            Event::BlockInhibited(inhibited) => {
                log::info!("{inhibited}");
                if inhibited != self.inhibitors.systemd_inhibitor {
                    let action = if inhibited { "Added" } else { "Removed" };
                    log::info!("{action} dbus inhibitor");

                    self.inhibitors.systemd_inhibitor = inhibited;
                    self.reset_idle_timers();
                }
            }
            #[cfg(feature = "audio")]
            Event::AudioInhibit(inhibited) => {
                self.inhibitors.audio_inhibitor = inhibited;
                self.reset_idle_timers();
            }
            Event::SessionLocked(locked) => {
                let cmd = if locked {
                    self.lock_cmd.as_ref()
                } else {
                    self.unlock_cmd.as_ref()
                };

                if let Some(cmd) = cmd {
                    let cmd = cmd.clone();
                    execute_command(cmd);
                }

                if locked {
                    self.state.set_lock_state(LockState::Locked);
                    if self.state.notification.is_none() {
                        self.state.notification =
                            Some(
                                self.notifier
                                    .get_idle_notification(0, &self.seat, &self.qh, ()),
                            );
                    }
                } else {
                    self.state.set_lock_state(LockState::Unlocked);
                    if let Some(notification) = self.state.notification.take() {
                        notification.destroy();
                    }
                }
            }
            Event::ScreenSaverLock => {
                if let Some(lock_cmd) = self.lock_cmd.as_ref() {
                    let lock_cmd = lock_cmd.clone();
                    execute_command(lock_cmd);
                    self.state.set_lock_state(LockState::Locked);
                    if self.state.notification.is_none() {
                        self.state.notification =
                            Some(
                                self.notifier
                                    .get_idle_notification(0, &self.seat, &self.qh, ()),
                            );
                    }
                }
            }
            Event::PrepareForSleep(sleep) => {
                let cmd = if sleep {
                    self.before_sleep_cmd.as_ref()
                } else {
                    self.after_sleep_cmd.as_ref()
                };

                if let Some(cmd) = cmd {
                    let cmd = cmd.clone();
                    execute_command(cmd);
                }
            }
        }
    }

    fn reset_idle_timers(&mut self) {
        self.listeners.iter_mut().for_each(|handler| {
            let current_met = if !self.inhibitors.active() {
                handler
                    .config
                    .conditions
                    .iter()
                    .all(|condition| match condition {
                        Condition::OnBattery => self.power.source() == &PowerSource::Battery,
                        Condition::OnAc => self.power.source() == &PowerSource::Plugged,
                        Condition::BatteryBelow(battery) => {
                            self.power.level_cmp(battery) == LevelComparison::Below
                        }
                        Condition::BatteryAbove(battery) => {
                            self.power.level_cmp(battery) == LevelComparison::Above
                        }
                        Condition::BatteryEqual(battery) => {
                            self.power.level_cmp(battery) == LevelComparison::Equal
                        }
                        Condition::BatteryLevel(level) => self.power.level() == level,
                        Condition::BatteryState(state) => self.power.state() == state,
                        Condition::UsbPlugged(id) => {
                            self.usb_context
                                .as_ref()
                                .and_then(|ctx| ctx.devices().ok())
                                .is_some_and(|devices| {
                                    devices.iter().any(|device| {
                                        let desc = device.device_descriptor().unwrap();
                                        format!("{:04x}:{:04x}", desc.vendor_id(), desc.product_id()) == **id
                                    })
                                })
                        }
                        Condition::UsbUnplugged(id) => {
                            self.usb_context
                                .as_ref()
                                .and_then(|ctx| ctx.devices().ok())
                                .is_some_and(|devices| {
                                    devices.iter().all(|device| {
                                        let desc = device.device_descriptor().unwrap();
                                        format!("{:04x}:{:04x}", desc.vendor_id(), desc.product_id()) != **id
                                    })
                                })
                        }
                    })
            } else {
                false
            };

            if current_met {
                if handler.notification.is_none() {
                    handler.notification = Some(self.notifier.get_idle_notification(
                        handler.config.timeout_millis(),
                        &self.seat,
                        &self.qh,
                        (),
                    ));

                    log::info!(
                        "Notification created\ntimeout: {}\nconditions: {:?}\non_timeout: {:?}\non_resume: {:?}",
                        handler.config.timeout,
                        handler.config.conditions,
                        handler.config.on_timeout,
                        handler.config.on_resume
                    );
                }
            } else if let Some(notification) = handler.notification.take() {
                notification.destroy();
                log::info!(
                    "Notification destroyed\ntimeout: {}\nconditions: {:?}\non_timeout: {:?}\non_resume: {:?}",
                    handler.config.timeout,
                    handler.config.conditions,
                    handler.config.on_timeout,
                    handler.config.on_resume
                );
            }
        });
    }
}

enum Event {
    GetActiveTime(oneshot::Sender<u32>),
    GetLockState(oneshot::Sender<LockState>),
    BatteryState(BatteryState),
    BatteryLevel(BatteryLevel),
    OnBattery(bool),
    BatteryPercentage(f64),
    ScreenSaverInhibit(bool),
    SimulateUserActivity,
    SessionLocked(bool),
    ScreenSaverLock,
    BlockInhibited(bool),
    PrepareForSleep(bool),
    Usb,
    #[cfg(feature = "audio")]
    AudioInhibit(bool),
}

fn execute_command(command: Arc<str>) {
    let mut child = match Command::new("/bin/sh")
        .arg("-c")
        .arg(command.as_ref())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            log::error!("failed to execute command '{command}': {err}");
            return;
        }
    };

    std::thread::spawn(move || match child.wait() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            log::error!("command '{command}' failed with exit status {status}")
        }
        Err(err) => log::error!("failed to wait on command '{command}': {err}"),
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
        // This is for detecting when session is resumed after being locked externally
        if let Some(notification) = state.state.notification.take() {
            if let ext_idle_notification_v1::Event::Resumed = event {
                state.state.set_lock_state(LockState::Unlocked);
                return;
            }
            state.state.notification = Some(notification);
        }

        let Some(handler) = state
            .listeners
            .iter()
            .find(|timeout| timeout.notification.as_ref() == Some(notification))
        else {
            return;
        };

        match event {
            ext_idle_notification_v1::Event::Idled => {
                if let Some(cmd) = handler.on_timeout() {
                    log::info!("Executing timeout command: {cmd}");
                    execute_command(cmd.clone());
                }
                state.state.set_lock_state(LockState::Locked);
            }
            ext_idle_notification_v1::Event::Resumed => {
                if let Some(cmd) = handler.on_resume() {
                    log::info!("Executing resume command: {cmd}");
                    execute_command(cmd.clone());
                }
                state.state.set_lock_state(LockState::Unlocked);
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

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[arg(short, long, action = clap::ArgAction::Count)]
    quiet: u8,

    #[arg(short, long, value_name = "FILE", help = "Path to the config file")]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let mut log_level = LevelFilter::Info;

    (0..cli.verbose).for_each(|_| {
        log_level = match log_level {
            LevelFilter::Error => LevelFilter::Warn,
            LevelFilter::Warn => LevelFilter::Info,
            LevelFilter::Info => LevelFilter::Debug,
            LevelFilter::Debug => LevelFilter::Trace,
            _ => log_level,
        };
    });

    (0..cli.quiet).for_each(|_| {
        log_level = match log_level {
            LevelFilter::Warn => LevelFilter::Error,
            LevelFilter::Info => LevelFilter::Warn,
            LevelFilter::Debug => LevelFilter::Info,
            LevelFilter::Trace => LevelFilter::Debug,
            _ => log_level,
        };
    });

    Builder::new().filter(Some("moxidle"), log_level).init();

    let conn = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let mut event_loop = EventLoop::try_new()?;
    let (emit_sender, emit_receiver) = mpsc::channel();
    let usb_context = rusb::Context::new();
    let mut moxidle = Moxidle::new(globals, qh, cli.config, emit_sender, usb_context.ok())?;

    WaylandSource::new(conn, event_queue).insert(event_loop.handle())?;

    let (executor, scheduler) = calloop::futures::executor()?;
    let (event_sender, event_receiver) = calloop::channel::channel();

    let dbus_conn = Arc::new(zbus::Connection::system().await?);
    {
        let ignore_on_battery = moxidle.should_ignore(|c| *c == Condition::OnBattery);
        let ignore_battery_percentage = moxidle.should_ignore(|c| {
            matches!(
                c,
                Condition::BatteryBelow(_)
                    | Condition::BatteryAbove(_)
                    | Condition::BatteryEqual(_)
            )
        });
        let ignore_battery_state =
            moxidle.should_ignore(|c| matches!(c, Condition::BatteryState(_)));
        let ignore_battery_level =
            moxidle.should_ignore(|c| matches!(c, Condition::BatteryLevel(_)));

        let event_sender = event_sender.clone();
        let dbus_conn = Arc::clone(&dbus_conn);
        scheduler.schedule(async move {
            if let Err(e) = upower::serve(
                dbus_conn,
                event_sender,
                ignore_on_battery,
                ignore_battery_percentage,
                ignore_battery_state,
                ignore_battery_level,
            )
            .await
            {
                log::error!("D-Bus upower error: {e}");
            }
        })?;
    }

    {
        let ignore_dbus_inhibit = moxidle.ignore_dbus_inhibit;
        let event_sender = event_sender.clone();
        scheduler.schedule(async move {
            if let Err(e) =
                screensaver::serve(event_sender, emit_receiver, ignore_dbus_inhibit).await
            {
                log::error!("D-Bus screensaver error: {e}");
            }
        })?;
    }

    {
        let ignore_systemd_inhibit = moxidle.ignore_systemd_inhibit;
        let event_sender = event_sender.clone();
        let dbus_conn = Arc::clone(&dbus_conn);
        scheduler.schedule(async move {
            if let Err(e) = login::serve(dbus_conn, event_sender, ignore_systemd_inhibit).await {
                log::error!("D-Bus login manager error: {e}");
            }
        })?;
    }

    #[cfg(feature = "audio")]
    {
        let ignore_audio_inhibit = moxidle.ignore_audio_inhibit;
        let event_sender = event_sender.clone();
        scheduler.schedule(async move {
            if let Err(e) = audio::serve(event_sender, ignore_audio_inhibit).await {
                log::error!("Audio error: {e}");
            }
        })?;
    }

    if let Some(usb_context) = moxidle.usb_context.as_ref() {
        let event_sender = event_sender.clone();
        usb::serve(event_sender, usb_context.clone())?;

        let usb_context = usb_context.clone();
        event_loop
            .handle()
            .insert_source(calloop::timer::Timer::immediate(), move |_, _, _| {
                if let Err(e) = usb_context.handle_events(None) {
                    log::error!("USB event handling error: {e}");
                }

                calloop::timer::TimeoutAction::ToInstant(
                    std::time::Instant::now() + std::time::Duration::from_millis(100),
                )
            })
            .map_err(|e| anyhow::anyhow!("Failed to insert USB event source: {e}"))?;
    }

    event_loop
        .handle()
        .insert_source(executor, |_: (), _, _| ())
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    event_loop
        .handle()
        .insert_source(event_receiver, |event, _, state| {
            if let calloop::channel::Event::Msg(event) = event {
                state.handle_app_event(event);
            }
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    event_loop.run(None, &mut moxidle, |_| {})?;
    Ok(())
}
