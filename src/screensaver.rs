// https://specifications.freedesktop.org/idle-inhibit-spec/latest
// https://invent.kde.org/plasma/kscreenlocker/-/blob/master/dbus/org.freedesktop.ScreenSaver.xml

use crate::{Event, LockState, State};
use calloop::channel;
use futures_lite::StreamExt;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use tokio::sync::{Mutex, RwLock};
use zbus::object_server::SignalEmitter;

#[derive(Debug)]
pub struct Inhibitor {
    cookie: u32,
    application_name: Box<str>,
    reason_for_inhibit: Box<str>,
    client: zbus::names::UniqueName<'static>,
}

#[derive(Clone)]
struct ScreenSaver {
    state: Arc<RwLock<State>>,
    inhibitors: Arc<Mutex<Vec<Inhibitor>>>,
    last_cookie: Arc<AtomicU32>,
    event_sender: channel::Sender<Event>,
}

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl ScreenSaver {
    #[zbus(signal)]
    async fn active_changed(&mut self, signal_emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    async fn lock(&self) {
        log::info!("Sending SessionLocked(true) event");
        if let Err(e) = self.event_sender.send(Event::ScreenSaverLock) {
            log::error!("Failed to send SessionLocked(true) event: {}", e);
        }
    }

    async fn simulate_user_activity(&self) {
        log::info!("Sending SimulateUserActivity event");
        if let Err(e) = self.event_sender.send(Event::SimulateUserActivity) {
            log::error!("Failed to send SimulateUserActivity event: {}", e);
        }
    }

    async fn get_active(&self) -> bool {
        self.state.read().await.lock_state == LockState::Locked
    }

    async fn get_active_time(&self) -> u32 {
        if let Some(time) = self.state.read().await.active_since {
            time.elapsed().as_secs() as u32
        } else {
            0
        }
    }

    async fn get_session_idle_time(&self) -> zbus::fdo::Result<u32> {
        Err(zbus::fdo::Error::ZBus(zbus::Error::Unsupported))
    }

    async fn set_active(&self, state: bool) -> bool {
        if state {
            self.lock().await;
        }

        state
    }

    async fn inhibit(
        &mut self,
        application_name: &str,
        reason_for_inhibit: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> u32 {
        let cookie = self.last_cookie.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(sender) = header.sender() {
            log::info!(
                "Added screensaver inhibitor for application '{}' {:?}, reason: {}, cookie: {}",
                application_name,
                sender,
                reason_for_inhibit,
                cookie
            );
            let mut inhibitors = self.inhibitors.lock().await;
            if inhibitors.is_empty() {
                log::info!("Sending ScreenSaverInhibit(true) event");
                if let Err(e) = self.event_sender.send(Event::ScreenSaverInhibit(true)) {
                    log::error!("Failed to send ScreenSaverInhibit event {}", e);
                }
            }
            inhibitors.push(Inhibitor {
                cookie,
                application_name: application_name.into(),
                reason_for_inhibit: reason_for_inhibit.into(),
                client: sender.to_owned(),
            });
        }
        cookie
    }

    async fn un_inhibit(&mut self, cookie: u32) {
        let mut inhibitors = self.inhibitors.lock().await;
        if let Some(idx) = inhibitors.iter().position(|x| x.cookie == cookie) {
            let inhibitor = inhibitors.remove(idx);
            if inhibitors.is_empty() {
                log::info!("Sending ScreenSaverInhibit(false) event");
                if let Err(e) = self.event_sender.send(Event::ScreenSaverInhibit(false)) {
                    log::error!("Failed to send ScreenSaverInhibit(false) event {}", e);
                }
            }
            log::info!(
                "Removed screensaver inhibitor for application '{}' {:?}, reason: {}, cookie: {}",
                inhibitor.application_name,
                inhibitor.client,
                inhibitor.reason_for_inhibit,
                inhibitor.cookie
            );
        }
    }

    async fn throttle(
        &mut self,
        application_name: &str,
        reason_for_inhibit: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> u32 {
        // TODO
        _ = header;
        _ = application_name;
        _ = reason_for_inhibit;

        0
    }

    async fn un_throttle(&mut self, cookie: u32) {
        // TODO
        _ = cookie;
    }
}

pub async fn serve(
    event_sender: channel::Sender<Event>,
    ignore_dbus_inhibit: bool,
    state: Arc<RwLock<State>>,
) -> zbus::Result<()> {
    if ignore_dbus_inhibit {
        return Ok(());
    }

    let inhibitors = Arc::new(Mutex::new(Vec::new()));

    let screensaver = ScreenSaver {
        state,
        inhibitors: Arc::clone(&inhibitors),
        event_sender: event_sender.clone(),
        last_cookie: Arc::new(AtomicU32::new(0)),
    };

    let conn = zbus::connection::Builder::session()?
        .serve_at("/ScreenSaver", screensaver.clone())?
        .serve_at("/org/freedesktop/ScreenSaver", screensaver.clone())?
        .build()
        .await?;

    conn.request_name_with_flags(
        "org.freedesktop.ScreenSaver",
        zbus::fdo::RequestNameFlags::ReplaceExisting.into(),
    )
    .await?;

    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
    let mut name_owner_stream = dbus.receive_name_owner_changed().await?;
    while let Some(event) = name_owner_stream.next().await {
        if let Ok(args) = event.args() {
            if args.new_owner.is_none() {
                if let zbus::names::BusName::Unique(name) = args.name {
                    let mut inhibitors = inhibitors.lock().await;
                    if !inhibitors.is_empty() {
                        inhibitors.retain(|inhibitor| inhibitor.client != name);
                        if inhibitors.is_empty() {
                            log::info!("Sending ScreenSaverInhibit(false) event");
                            if let Err(e) = event_sender.send(Event::ScreenSaverInhibit(false)) {
                                log::error!(
                                    "Failed to send ScreenSaverInhibit(false) event: {}",
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
