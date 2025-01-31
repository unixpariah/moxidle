use crate::Event;
use calloop::channel;
use futures_lite::StreamExt;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
use tokio::sync::Mutex;
use zbus::names::BusName;

#[derive(Debug)]
pub struct Inhibitor {
    cookie: u32,
    application_name: String,
    reason_for_inhibit: String,
    client: zbus::names::UniqueName<'static>,
}

#[derive(Clone)]
struct Screensaver {
    inhibitors: Arc<Mutex<Vec<Inhibitor>>>,
    last_cookie: Arc<AtomicU32>,
    event_sender: channel::Sender<Event>,
}

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl Screensaver {
    async fn inhibit(
        &mut self,
        application_name: String,
        reason_for_inhibit: String,
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
                if let Err(e) = self.event_sender.send(Event::ScreensaverInhibit(true)) {
                    log::warn!("Failed to send screensaver event: {}", e)
                }
            }
            inhibitors.push(Inhibitor {
                cookie,
                application_name,
                reason_for_inhibit,
                client: sender.to_owned(),
            });
        } else {
            log::warn!("Inhibit call without sender")
        }
        cookie
    }

    async fn un_inhibit(&mut self, cookie: u32) {
        let mut inhibitors = self.inhibitors.lock().await;
        if let Some(idx) = inhibitors.iter().position(|x| x.cookie == cookie) {
            let inhibitor = inhibitors.remove(idx);
            if inhibitors.is_empty() {
                if let Err(e) = self.event_sender.send(Event::ScreensaverInhibit(false)) {
                    log::warn!("Failed to send screensaver event: {}", e)
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
}

pub async fn serve(
    event_sender: channel::Sender<Event>,
    ignore_dbus_inhibit: bool,
) -> zbus::Result<()> {
    if ignore_dbus_inhibit {
        return Ok(());
    }

    let inhibitors = Arc::new(Mutex::new(Vec::new()));

    let screensaver = Screensaver {
        inhibitors: inhibitors.clone(),
        event_sender: event_sender.clone(),
        last_cookie: Arc::new(AtomicU32::new(0)),
    };

    let conn = zbus::connection::Builder::session()?
        .serve_at("/ScreenSaver", screensaver.clone())?
        .serve_at("/org/freedesktop/ScreenSaver", screensaver)?
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
        let args = event.args()?;
        if args.new_owner.is_none() {
            if let BusName::Unique(name) = args.name {
                let mut inhibitors = inhibitors.lock().await;
                let initial_count = inhibitors.len();
                inhibitors.retain(|inhibitor| inhibitor.client != name);

                let removed_count = initial_count - inhibitors.len();
                if removed_count > 0 {
                    log::info!(
                        "Removed {} inhibitors due to client disconnection: {:?}",
                        removed_count,
                        name
                    );

                    if inhibitors.is_empty() {
                        if let Err(e) = event_sender.send(Event::ScreensaverInhibit(false)) {
                            log::warn!("Failed to send screensaver event: {}", e);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
