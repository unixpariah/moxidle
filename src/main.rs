use calloop::{generic::Generic, EventLoop, PostAction};
use calloop_wayland_source::WaylandSource;
use nix::sys::{signal, signalfd};
use std::{rc, time};
use wayland_client::{
    protocol::{wl_registry, wl_seat},
    Connection, Dispatch, QueueHandle,
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

struct Moxidle {
    qh: QueueHandle<Self>,
    seat: Seat,
    notifier: Option<ext_idle_notifier_v1::ExtIdleNotifierV1>,
    idle_cmd: Option<rc::Rc<str>>,
    before_sleep_cmd: Option<rc::Rc<str>>,
    after_resume_cmd: Option<rc::Rc<str>>,
    logind_lock_cmd: Option<rc::Rc<str>>,
    logind_unlock_cmd: Option<rc::Rc<str>>,
    logind_idlehint: bool,
    timeouts_enabled: bool,
    wait: bool,
    timeout_cmds: Vec<TimeoutCmd>,
}

impl Moxidle {
    fn new(qh: QueueHandle<Self>) -> Self {
        Self {
            qh,
            seat: Seat::default(),
            notifier: None,
            idle_cmd: None,
            before_sleep_cmd: None,
            after_resume_cmd: None,
            logind_lock_cmd: None,
            logind_unlock_cmd: None,
            logind_idlehint: false,
            timeouts_enabled: false,
            wait: false,
            timeout_cmds: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct TimeoutCmd {
    idle_cmd: rc::Rc<str>,
    resume_cmd: rc::Rc<str>,
    idlehint: bool,
    resume_pending: bool,
    notification: Option<ext_idle_notification_v1::ExtIdleNotificationV1>,
    timeout: time::Duration,
    registered_timeout: time::Duration,
}

impl TimeoutCmd {
    fn handle_resumed(
        &mut self,
        notifier: &ext_idle_notifier_v1::ExtIdleNotifierV1,
        seat: &wl_seat::WlSeat,
        qh: &QueueHandle<Moxidle>,
    ) {
        self.resume_pending = false;
        log::debug!("active state");
        if self.registered_timeout != self.timeout {
            self.register_timeout(notifier, seat, qh);
        }
    }

    fn handle_idled(&mut self) {
        self.resume_pending = true;
        log::debug!("idle state");
    }

    fn register_timeout(
        &mut self,
        notifier: &ext_idle_notifier_v1::ExtIdleNotifierV1,
        seat: &wl_seat::WlSeat,
        qh: &QueueHandle<Moxidle>,
    ) {
        self.timer_destroy();
        if self.timeout.is_zero() {
            log::debug!("Not registering idle timeout");
            return;
        }

        log::debug!("Register with timeout: {}", self.timeout.as_secs());
        notifier.get_idle_notification(self.timeout.as_secs() as u32, seat, qh, ());

        self.registered_timeout = self.timeout;
    }

    fn timer_destroy(&mut self) {
        if let Some(notification) = &self.notification {
            notification.destroy();
            self.notification = None;
        }
    }
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
                    //state.notification = Some(
                    //    registry.bind::<ext_idle_notification_v1::ExtIdleNotificationV1, _, _>(
                    //        name,
                    //        version,
                    //        qh,
                    //        (),
                    //    ),
                    //)
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name: _ } => {}
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Moxidle {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: <wl_seat::WlSeat as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            wl_seat::Event::Name { name } => state.seat.name = name,
            wl_seat::Event::Capabilities { capabilities } => {
                state.seat.capabilities = Some(capabilities)
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for Moxidle {
    fn event(
        state: &mut Self,
        notification: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: <ext_idle_notification_v1::ExtIdleNotificationV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
        let Some(timeout_cmd) = state
            .timeout_cmds
            .iter_mut()
            .find(|timeout_cmd| timeout_cmd.notification.as_ref() == Some(notification))
        else {
            return;
        };

        let Some(notifier) = state.notifier.as_ref() else {
            return;
        };
        let Some(seat) = state.seat.proxy.as_ref() else {
            return;
        };

        match event {
            ext_idle_notification_v1::Event::Idled => timeout_cmd.handle_idled(),
            ext_idle_notification_v1::Event::Resumed => {
                timeout_cmd.handle_resumed(notifier, seat, &state.qh)
            }
            _ => {}
        };
    }
}

impl Dispatch<ext_idle_notifier_v1::ExtIdleNotifierV1, ()> for Moxidle {
    fn event(
        _: &mut Self,
        _: &ext_idle_notifier_v1::ExtIdleNotifierV1,
        _: <ext_idle_notifier_v1::ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

fn handle_signal(signal: i32, moxidle: &mut Moxidle) {
    let signal = unsafe { std::mem::transmute::<i32, signal::Signal>(signal) };
    match signal {
        signal::Signal::SIGINT => {}
        signal::Signal::SIGTERM => moxidle.timeout_cmds.iter_mut().for_each(|cmd| {
            if cmd.resume_pending {
                cmd.handle_resumed(
                    moxidle.notifier.as_ref().unwrap(),
                    moxidle.seat.proxy.as_ref().unwrap(),
                    &moxidle.qh,
                );
            }
        }),
        signal::Signal::SIGUSR1 => moxidle.timeout_cmds.iter_mut().for_each(|cmd| {
            cmd.register_timeout(
                moxidle.notifier.as_ref().unwrap(),
                moxidle.seat.proxy.as_ref().unwrap(),
                &moxidle.qh,
            );
        }),
        _ => unreachable!(),
    }
}

fn main() {
    env_logger::init();
    // load config
    // parse args

    let mut event_loop: EventLoop<Moxidle> = EventLoop::try_new().unwrap();
    let loop_handle = event_loop.handle();

    let mut sigset = signalfd::SigSet::empty();
    sigset.add(signal::SIGINT);
    sigset.add(signal::SIGTERM);
    sigset.add(signal::SIGUSR1);
    sigset.thread_block().expect("Failed to block signals");

    let signal_fd = signalfd::SignalFd::new(&sigset).expect("Failed to create signalfd");

    _ = loop_handle.insert_source(
        Generic::new(signal_fd, calloop::Interest::READ, calloop::Mode::Level),
        |_, signal_fd, moxidle| {
            let signal_info = signal_fd.read_signal().expect("Failed to read signal");
            if let Some(signal) = signal_info {
                handle_signal(signal.ssi_signo as i32, moxidle);
            }
            Ok(PostAction::Continue)
        },
    );

    let conn = Connection::connect_to_env().expect("Unable to connect to the compositor");
    let display = conn.display();

    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    _ = display.get_registry(&qh, ());
    let mut moxidle = Moxidle::new(qh);
    event_queue.roundtrip(&mut moxidle).unwrap();
    event_queue.roundtrip(&mut moxidle).unwrap();

    if moxidle.notifier.is_none() {
        log::error!("Failed to bind ext-idle-notifier-v1.");
        return;
    }

    let mut should_run = !moxidle.timeout_cmds.is_empty();

    if cfg!(feature = "systemd") || cfg!(feature = "elogind") {
        // connect to bus
        // setup property changed listener
        if moxidle.before_sleep_cmd.is_some() {
            should_run = true;
            // setup sleep listener
        }
        if moxidle.logind_lock_cmd.is_some() {
            should_run = true;
            // setup lock listener
        }
        if moxidle.logind_unlock_cmd.is_some() {
            should_run = true;
            // setup unlock listener
        }
        if moxidle.logind_idlehint {
            should_run = true;
            // set idle hint
        }
    }

    if !should_run {
        log::info!("No command specified, exiting");
        // terminate
    }

    // enable timeouts

    event_queue.roundtrip(&mut moxidle).unwrap();
    WaylandSource::new(conn, event_queue)
        .insert(loop_handle)
        .unwrap();

    event_loop
        .run(None, &mut moxidle, |_| {})
        .expect("Event loop failed");
}
