mod audio;
mod config;
mod login;
mod screensaver;
mod upower;

use calloop::{channel, EventLoop};
use calloop_wayland_source::WaylandSource;
use clap::Parser;
use config::Condition;
use config::{Config, MoxidleConfig, TimeoutConfig};
use env_logger::Builder;
use log::LevelFilter;
use std::{error::Error, ops::Deref, path::PathBuf, sync::Arc, time::Instant};
use tokio::process::Command;
use tokio::sync::RwLock;
use upower::{BatteryLevel, BatteryState, LevelComparison, Power, PowerSource};
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
    notification: Option<ext_idle_notification_v1::ExtIdleNotificationV1>,
}

impl Deref for TimeoutHandler {
    type Target = TimeoutConfig;

    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

impl TimeoutHandler {
    fn new(config: TimeoutConfig) -> Self {
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
    audio_inhibitor: bool,
    dbus_inhibitor: bool,
    systemd_inhibitor: bool,
}

impl Inhibitors {
    fn active(&self) -> bool {
        self.dbus_inhibitor || self.audio_inhibitor || self.systemd_inhibitor
    }
}

#[derive(PartialEq)]
enum LockState {
    Locked,
    Unlocked,
}

struct State {
    lock_state: LockState,
    active_since: Option<Instant>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            active_since: None,
            lock_state: LockState::Unlocked,
        }
    }
}

struct Moxidle {
    state: Arc<RwLock<State>>,
    seat: wl_seat::WlSeat,
    notifier: ext_idle_notifier_v1::ExtIdleNotifierV1,
    timeouts: Vec<TimeoutHandler>,
    config: MoxidleConfig,
    inhibitors: Inhibitors,
    qh: QueueHandle<Self>,
    power: Power,
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
    ) -> Result<Self> {
        let notifier = globals
            .bind(&qh, 1..=1, ())
            .expect("Compositor doesn't support ext-idle-notifier-v1");

        let seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=4, ())?;
        seat.get_pointer(&qh, ());

        let (general_config, timeout_configs) = Config::load(config_path)?;

        let timeouts = timeout_configs
            .into_iter()
            .map(TimeoutHandler::new)
            .collect();

        Ok(Self {
            state: Arc::new(RwLock::new(State::default())),
            power: Power::default(),
            timeouts,
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
            .timeouts
            .iter()
            .any(|timeout| timeout.config.conditions.iter().any(&condition_predicate))
    }

    async fn handle_app_event(&mut self, event: Event) {
        match event {
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
            Event::SimulateUserActivity => {
                self.reset_idle_timers();
            }
            Event::ScreenSaverInhibit(inhibited) => {
                self.inhibitors.dbus_inhibitor = inhibited;
                self.reset_idle_timers();
            }
            Event::AudioInhibit(inhibited) => {
                self.inhibitors.audio_inhibitor = inhibited;
                self.reset_idle_timers();
            }
            Event::BlockInhibited(inhibition) => {
                self.inhibitors.dbus_inhibitor = inhibition.contains("idle");
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
                    tokio::spawn(async move {
                        execute_command(cmd).await;
                    });
                }

                let mut state = self.state.write().await;
                if locked {
                    state.active_since.get_or_insert(Instant::now());
                    state.lock_state = LockState::Locked;
                } else {
                    state.active_since = None;
                    state.lock_state = LockState::Unlocked;
                }
            }
            Event::ScreenSaverLock => {
                if let Some(lock_cmd) = self.lock_cmd.as_ref() {
                    let lock_cmd = lock_cmd.clone();
                    tokio::spawn(async move {
                        execute_command(lock_cmd).await;
                    });
                    let mut state = self.state.write().await;
                    if state.active_since.is_none() {
                        state.active_since = Some(Instant::now());
                    }
                    state.lock_state = LockState::Locked;
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
                    tokio::spawn(async move {
                        execute_command(cmd).await;
                    });
                }
            }
        }
    }

    fn reset_idle_timers(&mut self) {
        let power = &self.power;
        self.timeouts.iter_mut().for_each(|handler| {
            let current_met = if !self.inhibitors.active() {
                handler.conditions.iter().all(|condition| match condition {
                    Condition::OnBattery => power.source() == &PowerSource::Battery,
                    Condition::OnAc => power.source() == &PowerSource::Plugged,
                    Condition::BatteryBelow(battery) => {
                        power.level_cmp(battery) == LevelComparison::Below
                    }
                    Condition::BatteryAbove(battery) => {
                        power.level_cmp(battery) == LevelComparison::Above
                    }
                    Condition::BatteryEqual(battery) => {
                        power.level_cmp(battery) == LevelComparison::Equal
                    }
                    Condition::BatteryLevel(level) => power.level() == level,
                    Condition::BatteryState(state) => power.state() == state,
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
                        "Notification created\ntimeout: {}\non_timeout: {:?}\non_resume: {:?}",
                        handler.timeout,
                        handler.on_timeout,
                        handler.on_resume
                    );
                }
            } else if let Some(notification) = handler.notification.take() {
                notification.destroy();
                log::info!(
                    "Notification destroyed\ntimeout: {}\non_timeout: {:?}\non_resume: {:?}",
                    handler.timeout,
                    handler.on_timeout,
                    handler.on_resume
                );
            }
        });
    }
}

enum Event {
    BatteryState(BatteryState),
    BatteryLevel(BatteryLevel),
    OnBattery(bool),
    BatteryPercentage(f64),
    ScreenSaverInhibit(bool),
    SimulateUserActivity,
    SessionLocked(bool),
    ScreenSaverLock,
    BlockInhibited(String),
    PrepareForSleep(bool),
    AudioInhibit(bool),
}

async fn execute_command(command: Arc<str>) {
    match Command::new("/bin/sh")
        .arg("-c")
        .arg(&*command)
        .status()
        .await
    {
        Ok(status) => {
            if !status.success() {
                log::error!("command '{}' failed with exit status {}", command, status);
            }
        }
        Err(e) => {
            log::error!("Command '{}' failed {}", command, e);
        }
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
        let Some(handler) = state
            .timeouts
            .iter()
            .find(|timeout| timeout.notification.as_ref() == Some(notification))
        else {
            return;
        };

        match event {
            ext_idle_notification_v1::Event::Idled => {
                let state = state.state.clone();
                if let Some(cmd) = handler.on_timeout() {
                    log::info!("Executing timeout command: {}", cmd);
                    let cmd = cmd.clone();
                    tokio::spawn(async move {
                        execute_command(cmd).await;
                        let mut state = state.write().await;
                        if state.active_since.is_none() {
                            state.active_since = Some(Instant::now());
                        }
                        state.lock_state = LockState::Locked;
                    });
                } else {
                    tokio::spawn(async move {
                        let mut state = state.write().await;
                        if state.active_since.is_none() {
                            state.active_since = Some(Instant::now());
                        }
                        state.lock_state = LockState::Locked;
                    });
                }
            }
            ext_idle_notification_v1::Event::Resumed => {
                let state = state.state.clone();
                if let Some(cmd) = handler.on_resume() {
                    log::info!("Executing resume command: {}", cmd);
                    let cmd = cmd.clone();
                    tokio::spawn(async move {
                        execute_command(cmd).await;
                        let mut state = state.write().await;
                        state.active_since = None;
                        state.lock_state = LockState::Unlocked;
                    });
                } else {
                    tokio::spawn(async move {
                        let mut state = state.write().await;
                        state.active_since = None;
                        state.lock_state = LockState::Unlocked;
                    });
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
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut log_level = LevelFilter::Error;

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

    Builder::new().filter_level(log_level).init();

    let conn = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let mut event_loop = EventLoop::try_new()?;
    let mut moxidle = Moxidle::new(globals, qh, cli.config)?;

    WaylandSource::new(conn, event_queue).insert(event_loop.handle())?;

    let (executor, scheduler) = calloop::futures::executor()?;
    let (event_sender, event_receiver) = channel::channel();

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
                log::error!("D-Bus upower error: {}", e);
            }
        })?;
    }

    {
        let state = moxidle.state.clone();
        let ignore_dbus_inhibit = moxidle.ignore_dbus_inhibit;
        let event_sender = event_sender.clone();
        scheduler.schedule(async move {
            if let Err(e) = screensaver::serve(event_sender, ignore_dbus_inhibit, state).await {
                log::error!("D-Bus screensaver error: {}", e);
            }
        })?;
    }

    {
        let ignore_systemd_inhibit = moxidle.ignore_systemd_inhibit;
        let event_sender = event_sender.clone();
        let dbus_conn = Arc::clone(&dbus_conn);
        scheduler.schedule(async move {
            if let Err(e) = login::serve(dbus_conn, event_sender, ignore_systemd_inhibit).await {
                log::error!("D-Bus login manager error: {}", e);
            }
        })?;
    }

    {
        let ignore_audio_inhibit = moxidle.ignore_audio_inhibit;
        let event_sender = event_sender.clone();
        scheduler.schedule(async move {
            if let Err(e) = audio::serve(event_sender, ignore_audio_inhibit).await {
                log::error!("Audio error: {}", e);
            }
        })?;
    }

    event_loop
        .handle()
        .insert_source(executor, |_: (), _, _| ())?;
    event_loop
        .handle()
        .insert_source(event_receiver, |event, _, state| {
            if let channel::Event::Msg(event) = event {
                pollster::block_on(state.handle_app_event(event));
            }
        })?;

    event_loop.run(None, &mut moxidle, |_| {})?;
    Ok(())
}
