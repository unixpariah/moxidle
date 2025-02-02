#[cfg(feature = "audio")]
mod audio;
mod config;
#[cfg(feature = "systemd")]
mod login;
#[cfg(feature = "dbus")]
mod screensaver;
#[cfg(feature = "upower")]
mod upower;

use calloop::{channel, EventLoop};
use calloop_wayland_source::WaylandSource;
use clap::Parser;
use config::{Condition, FullConfig, MoxidleConfig, TimeoutConfig};
use env_logger::Builder;
use log::LevelFilter;
use std::{error::Error, ops::Deref, path::PathBuf, process::Command, sync::Arc};
#[cfg(feature = "upower")]
use upower::Power;
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
    fn new(
        config: TimeoutConfig,
        qh: &QueueHandle<Moxidle>,
        seat: &wl_seat::WlSeat,
        notifier: &ext_idle_notifier_v1::ExtIdleNotifierV1,
    ) -> Self {
        let notification = notifier.get_idle_notification(config.timeout_millis(), seat, qh, ());

        Self {
            config,
            notification: Some(notification),
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
    #[cfg(feature = "upower")]
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

        let (general_config, timeout_configs) = FullConfig::load(config_path)?.split_into_parts();

        let timeouts = timeout_configs
            .into_iter()
            .map(|cfg| TimeoutHandler::new(cfg, &qh, &seat, &notifier))
            .collect();

        Ok(Self {
            #[cfg(feature = "upower")]
            power: Power::default(),
            timeouts,
            config: general_config,
            notifier,
            seat,
            inhibited: false,
            qh,
        })
    }

    fn handle_app_event(&mut self, event: Event) {
        match event {
            #[cfg(feature = "upower")]
            Event::OnBattery(on_battery) => {
                self.power.unplugged = on_battery;
                self.reset_idle_timers();
            }
            #[cfg(feature = "upower")]
            Event::BatteryPercentage(battery) => {
                self.power.percentage = battery;
                self.reset_idle_timers();
            }
            #[cfg(feature = "dbus")]
            Event::ScreensaverInhibit(inhibited) => {
                self.inhibited = inhibited;
                self.reset_idle_timers();
            }
            #[cfg(feature = "audio")]
            Event::AudioInhibit(inhibited) => {
                self.inhibited = inhibited;
                self.reset_idle_timers();
            }
            #[cfg(feature = "systemd")]
            Event::BlockInhibited(inhibition) => {
                if inhibition.contains("idle") {
                    if !self.inhibited {
                        self.inhibited = true;
                        log::info!("systemd idle inhibit active");
                    }
                    self.reset_idle_timers();
                } else {
                    self.inhibited = false;
                    self.reset_idle_timers();
                }
            }
            #[cfg(feature = "systemd")]
            Event::SessionLocked(locked) => {
                if locked {
                    if let Some(lock_cmd) = self.lock_cmd.as_ref() {
                        execute_command(lock_cmd.clone());
                    }
                } else if let Some(unlock_cmd) = self.unlock_cmd.as_ref() {
                    execute_command(unlock_cmd.clone());
                }
            }
            #[cfg(feature = "systemd")]
            Event::PrepareForSleep(sleep) => {
                if sleep {
                    if let Some(before_sleep_cmd) = self.before_sleep_cmd.as_ref() {
                        execute_command(before_sleep_cmd.clone());
                    } else if let Some(after_sleep_cmd) = self.after_sleep_cmd.as_ref() {
                        execute_command(after_sleep_cmd.clone());
                    }
                }
            }
        }
    }

    fn reset_idle_timers(&mut self) {
        self.timeouts.iter_mut().for_each(|handler| {
            let current_met = if !self.inhibited {
                handler.conditions.iter().all(|condition| {
                    #[cfg(feature = "upower")]
                    match condition {
                        Condition::OnBattery => self.power.unplugged,
                        Condition::OnAc => !self.power.unplugged,
                        Condition::BatteryBelow(battery) => self.power.percentage.lt(battery),
                        Condition::BatteryAbove(battery) => self.power.percentage.gt(battery),
                    }
                    #[cfg(not(feature = "upower"))]
                    true
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
                        "Notification created\ntimeout: {}\ncommand: {:?}",
                        handler.timeout,
                        handler.on_timeout
                    );
                }
            } else if let Some(notification) = handler.notification.take() {
                notification.destroy();
                log::info!(
                    "Notification destroyed\ntimeout: {}\ncommand: {:?}",
                    handler.timeout,
                    handler.on_timeout
                );
            }
        });
    }
}

#[derive(Debug)]
enum Event {
    #[cfg(feature = "upower")]
    OnBattery(bool),
    #[cfg(feature = "upower")]
    BatteryPercentage(f64),
    #[cfg(feature = "dbus")]
    ScreensaverInhibit(bool),
    #[cfg(feature = "systemd")]
    SessionLocked(bool),
    #[cfg(feature = "systemd")]
    BlockInhibited(String),
    #[cfg(feature = "systemd")]
    PrepareForSleep(bool),
    #[cfg(feature = "audio")]
    AudioInhibit(bool),
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
        let Some(handler) = state
            .timeouts
            .iter()
            .find(|timeout| timeout.notification.as_ref() == Some(notification))
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

    #[cfg(feature = "dbus")]
    let dbus_conn = Arc::new(zbus::Connection::system().await?);
    #[cfg(feature = "upower")]
    {
        let ignore_on_battery = !moxidle.timeouts.iter().any(|timeout| {
            timeout
                .config
                .conditions
                .iter()
                .any(|condition| *condition == Condition::OnBattery)
        });

        let ignore_battery_percentage = !moxidle.timeouts.iter().any(|timeout| {
            timeout.config.conditions.iter().any(|condition| {
                matches!(
                    condition,
                    Condition::BatteryBelow(_) | Condition::BatteryAbove(_)
                )
            })
        });

        let event_sender = event_sender.clone();
        let dbus_conn = Arc::clone(&dbus_conn);
        scheduler.schedule(async move {
            if let Err(e) = upower::serve(
                dbus_conn,
                event_sender,
                ignore_on_battery,
                ignore_battery_percentage,
            )
            .await
            {
                log::error!("D-Bus upower error: {}", e);
            }
        })?;
    }

    #[cfg(feature = "dbus")]
    {
        let ignore_dbus_inhibit = moxidle.ignore_dbus_inhibit;
        let event_sender = event_sender.clone();
        scheduler.schedule(async move {
            if let Err(e) = screensaver::serve(event_sender, ignore_dbus_inhibit).await {
                log::error!("D-Bus screensaver error: {}", e);
            }
        })?;
    }

    #[cfg(feature = "systemd")]
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

    #[cfg(feature = "audio")]
    {
        let ignore_audio_inhibit = moxidle.ignore_audio_inhibit;
        let event_sender = event_sender.clone();
        scheduler.schedule(async move {
            if let Err(e) = audio::serve(event_sender, ignore_audio_inhibit).await {
                log::error!("Audio error: {}", e);
            }
        })?;
    }

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
