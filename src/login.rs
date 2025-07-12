use crate::Event;
use calloop::channel;
use futures_lite::StreamExt;
use std::sync::Arc;

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait LoginManager {
    async fn get_session(&self, session_id: &str) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;

    #[zbus(property)]
    fn block_inhibited(&self) -> zbus::Result<String>;

    #[zbus(signal)]
    async fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
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
    if let Err(e) = sender.send(Event::BlockInhibited(value.contains("idle"))) {
        log::error!("Failed to send BlockInhibited event: {e}");
    }
}

pub async fn serve(
    connection: Arc<zbus::Connection>,
    event_sender: channel::Sender<Event>,
    ignore_systemd_inhibit: bool,
) -> zbus::Result<()> {
    let login_manager = Arc::new(LoginManagerProxy::new(&connection).await?);
    let session_path = login_manager.get_session("auto").await?;

    let login_session = match LoginSessionProxy::builder(&connection)
        .path(session_path)?
        .build()
        .await
    {
        Ok(session) => Arc::new(session),
        Err(e) => {
            log::error!("Couldn't create session proxy: {e}");
            return Ok(());
        }
    };

    if !ignore_systemd_inhibit {
        let event_sender = event_sender.clone();
        let login_manager = Arc::clone(&login_manager);
        tokio::spawn(async move {
            let mut block_inhibited_stream = login_manager.receive_block_inhibited_changed().await;
            while let Some(change) = block_inhibited_stream.next().await {
                if change.name() == "BlockInhibited"
                    && let Ok(block_inhibited) = change.get().await
                {
                    handle_block_inhibited(&block_inhibited, &event_sender).await;
                }
            }
        });
    }

    {
        let event_sender = event_sender.clone();
        let login_session = Arc::clone(&login_session);
        tokio::spawn(async move {
            let mut lock_stream = login_session.receive_lock().await.unwrap();
            while lock_stream.next().await.is_some() {
                if let Err(e) = event_sender.send(Event::SessionLocked(true)) {
                    log::error!("Failed to send SessionLocked event: {e}")
                }
            }
        });
    }

    {
        let event_sender = event_sender.clone();
        tokio::spawn(async move {
            let mut unlock_stream = login_session.receive_unlock().await.unwrap();
            while unlock_stream.next().await.is_some() {
                if let Err(e) = event_sender.send(Event::SessionLocked(false)) {
                    log::error!("Failed to send SessionLocked event: {e}")
                }
            }
        });
    }

    {
        let event_sender = event_sender.clone();
        tokio::spawn(async move {
            let mut sleep_stream = login_manager.receive_prepare_for_sleep().await.unwrap();
            while let Some(sleep) = sleep_stream.next().await {
                if let Ok(sleep) = sleep.args() {
                    let start = *sleep.start();
                    if let Err(e) = event_sender.send(Event::PrepareForSleep(start)) {
                        log::error!("Failed to send PrepareForSleep({start}) event: {e}")
                    }
                }
            }
        });
    }

    Ok(())
}
