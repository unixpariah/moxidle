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

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait LoginManager {
    async fn get_session(&self, session_id: &str) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;

    #[zbus(property)]
    fn block_inhibited(&self) -> zbus::Result<String>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1"
)]
trait LoginSession {
    #[zbus(signal)]
    fn lock(&self) -> zbus::Result<bool>;

    #[zbus(signal)]
    fn unlock(&self) -> zbus::Result<bool>;
}

async fn handle_block_inhibited(value: &str, sender: &channel::Sender<Event>) {
    if let Err(e) = sender.send(Event::BlockInhibited(value.to_string())) {
        log::warn!("Failed to send BlockInhibited event: {}", e);
    }
}

pub async fn serve(event_sender: channel::Sender<Event>) -> zbus::Result<()> {
    let inhibitors = Arc::new(Mutex::new(Vec::new()));

    let screensaver = Screensaver {
        inhibitors: inhibitors.clone(),
        event_sender: event_sender.clone(),
        last_cookie: Arc::new(AtomicU32::new(0)),
    };

    let system_conn = zbus::Connection::system().await?;
    let login_manager = LoginManagerProxy::new(&system_conn).await?;
    let session_path = login_manager.get_session("auto").await?;

    let login_session = match LoginSessionProxy::builder(&system_conn)
        .path(session_path)?
        .build()
        .await
    {
        Ok(session) => session,
        Err(e) => {
            log::warn!("Couldn't create session proxy: {}", e);
            return Ok(());
        }
    };

    if let Ok(block_inhibited) = login_manager.block_inhibited().await {
        handle_block_inhibited(&block_inhibited, &event_sender).await;
    }

    {
        let event_sender = event_sender.clone();
        let login_manager = login_manager.clone();
        tokio::spawn(async move {
            let mut stream = login_manager.receive_block_inhibited_changed().await;
            while let Some(change) = stream.next().await {
                println!("{}", change.name());
                if change.name() == "org.freedesktop.login1.Manager" {
                    if let Ok(block_inhibited) = change.get().await {
                        handle_block_inhibited(&block_inhibited, &event_sender).await;
                    }
                }
            }
        });
    }

    {
        let event_sender = event_sender.clone();
        let mut lock_stream = login_session.receive_lock().await?;
        tokio::spawn(async move {
            while lock_stream.next().await.is_some() {
                log::info!("Session lock requested");
                event_sender.send(Event::SessionLocked(true)).unwrap();
            }
        });
    }

    {
        let event_sender = event_sender.clone();
        let mut unlock_stream = login_session.receive_unlock().await?;
        tokio::spawn(async move {
            while unlock_stream.next().await.is_some() {
                log::info!("Session unlock requested");
                event_sender.send(Event::SessionLocked(false)).unwrap();
            }
        });
    }

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
