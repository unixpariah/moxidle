use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::Event;
use calloop::channel;
use futures_lite::StreamExt;

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
    if let Err(e) = sender.send(Event::BlockInhibited(value.to_string())) {
        log::warn!("Failed to send BlockInhibited event: {}", e);
    }
}

pub async fn serve(
    event_sender: channel::Sender<Event>,
    ignore_systemd_inhibit: Arc<AtomicBool>,
) -> zbus::Result<()> {
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

    if !ignore_systemd_inhibit.load(Ordering::SeqCst) {
        if let Ok(block_inhibited) = login_manager.block_inhibited().await {
            handle_block_inhibited(&block_inhibited, &event_sender).await;
        }

        {
            let event_sender = event_sender.clone();
            let login_manager = login_manager.clone();
            tokio::spawn(async move {
                let mut stream = login_manager.receive_block_inhibited_changed().await;
                while let Some(change) = stream.next().await {
                    if change.name() == "org.freedesktop.login1.Manager" {
                        if let Ok(block_inhibited) = change.get().await {
                            handle_block_inhibited(&block_inhibited, &event_sender).await;
                        }
                    }
                }
            });
        }
    }

    {
        let event_sender = event_sender.clone();
        let mut lock_stream = login_session.receive_lock().await?;
        tokio::spawn(async move {
            while lock_stream.next().await.is_some() {
                log::info!("Session lock requested");
                if let Err(e) = event_sender.send(Event::SessionLocked(true)) {
                    log::info!("Failed to get unlock args: {}", e)
                }
            }
        });
    }

    {
        let event_sender = event_sender.clone();
        let mut unlock_stream = login_session.receive_unlock().await?;
        tokio::spawn(async move {
            while unlock_stream.next().await.is_some() {
                log::info!("Session unlock requested");
                if let Err(e) = event_sender.send(Event::SessionLocked(false)) {
                    log::info!("Failed to get unlock args: {}", e)
                }
            }
        });
    }

    {
        let event_sender = event_sender.clone();
        let mut sleep_stream = login_manager.receive_prepare_for_sleep().await?;
        tokio::spawn(async move {
            while let Some(sleep) = sleep_stream.next().await {
                if let Ok(sleep) = sleep.args() {
                    log::info!("Prepare for sleep: {:?}", sleep.start());
                    if let Err(e) = event_sender.send(Event::PrepareForSleep(*sleep.start())) {
                        log::info!("Failed to get sleep args: {}", e)
                    }
                }
            }
        });
    }

    Ok(())
}
