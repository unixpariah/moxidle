// https://specifications.freedesktop.org/idle-inhibit-spec/latest
// https://invent.kde.org/plasma/kscreenlocker/-/blob/master/dbus/org.freedesktop.ScreenSaver.xml

use crate::{Event, LockState};
use calloop::channel;
use futures_lite::StreamExt;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    mpsc, Arc,
};
use tokio::sync::{oneshot, Mutex};
use zbus::object_server::SignalEmitter;

#[derive(Debug)]
struct Inhibitor {
    cookie: u32,
    application_name: Box<str>,
    reason_for_inhibit: Box<str>,
    client: zbus::names::UniqueName<'static>,
}

#[derive(Clone)]
struct ScreenSaver {
    inhibitors: Arc<Mutex<Vec<Inhibitor>>>,
    last_cookie: Arc<AtomicU32>,
    event_sender: channel::Sender<Event>,
}

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl ScreenSaver {
    #[zbus(signal)]
    async fn active_changed(signal_emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

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
        let (response_tx, response_rx) = oneshot::channel();
        if let Err(e) = self.event_sender.send(Event::GetLockState(response_tx)) {
            log::error!("Failed to send GetActiveTime request: {e}");
            return false;
        }
        response_rx.await.unwrap_or(LockState::Unlocked) == LockState::Locked
    }

    async fn get_active_time(&self) -> u32 {
        let (response_tx, response_rx) = oneshot::channel();
        if let Err(e) = self.event_sender.send(Event::GetActiveTime(response_tx)) {
            log::error!("Failed to send GetActiveTime request: {e}");
            return 0;
        }
        response_rx.await.unwrap_or(0)
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
        // TODO: If anyone tells me what it's supposed to do I'd be happy to implement
        _ = header;
        _ = application_name;
        _ = reason_for_inhibit;

        0
    }

    async fn un_throttle(&mut self, cookie: u32) {
        // TODO: If anyone tells me what it's supposed to do I'd be happy to implement
        _ = cookie;
    }
}

pub async fn serve(
    event_sender: channel::Sender<Event>,
    emit_receiver: mpsc::Receiver<()>,
    ignore_dbus_inhibit: bool,
) -> zbus::Result<()> {
    if ignore_dbus_inhibit {
        return Ok(());
    }

    let inhibitors = Arc::new(Mutex::new(Vec::new()));

    let screensaver = ScreenSaver {
        inhibitors: Arc::clone(&inhibitors),
        event_sender: event_sender.clone(),
        last_cookie: Arc::new(AtomicU32::new(0)),
    };

    let paths = ["/ScreenSaver", "/org/freedesktop/ScreenSaver"];
    let conn = paths
        .iter()
        .try_fold(zbus::connection::Builder::session()?, |builder, &path| {
            builder.serve_at(path, screensaver.clone())
        })?
        .build()
        .await?;

    conn.request_name_with_flags(
        "org.freedesktop.ScreenSaver",
        zbus::fdo::RequestNameFlags::ReplaceExisting.into(),
    )
    .await?;

    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
    let mut name_owner_stream = dbus.receive_name_owner_changed().await?;
    tokio::spawn(async move {
        while let Some(event) = name_owner_stream.next().await {
            if let Ok(args) = event.args() {
                if args.new_owner.is_none() {
                    if let zbus::names::BusName::Unique(name) = args.name {
                        let mut inhibitors = inhibitors.lock().await;
                        if !inhibitors.is_empty() {
                            inhibitors.retain(|inhibitor| inhibitor.client != name);
                            if inhibitors.is_empty() {
                                log::info!("Sending ScreenSaverInhibit(false) event");
                                if let Err(e) = event_sender.send(Event::ScreenSaverInhibit(false))
                                {
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
    });

    let interfaces = tokio::try_join!(
        conn.object_server()
            .interface::<_, ScreenSaver>("/org/freedesktop/ScreenSaver"),
        conn.object_server()
            .interface::<_, ScreenSaver>("/ScreenSaver"),
    )?;

    tokio::spawn(async move {
        loop {
            if let Err(e) = emit_receiver.recv() {
                log::error!("Failed to receive emit event: {e}");
            }

            if let Err(e) = tokio::try_join!(
                ScreenSaver::active_changed(interfaces.0.signal_emitter()),
                ScreenSaver::active_changed(interfaces.1.signal_emitter())
            ) {
                log::error!("Failed to emit active changed event: {}", e);
            }
        }
    });

    Ok(())
}
