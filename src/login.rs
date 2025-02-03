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
    log::info!("Sending BlockInhibited event");
    if let Err(e) = sender.send(Event::BlockInhibited(value.to_string())) {
        log::warn!("Failed to send BlockInhibited event: {}", e);
    }
}

pub async fn serve(
    connection: Arc<zbus::Connection>,
    event_sender: channel::Sender<Event>,
    ignore_systemd_inhibit: bool,
) -> zbus::Result<()> {
    let login_manager = LoginManagerProxy::new(&connection).await?;
    let session_path = login_manager.get_session("auto").await?;

    let login_session = match LoginSessionProxy::builder(&connection)
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

    let (mut block_inhibited_stream, mut lock_stream, mut unlock_stream, mut sleep_stream) = tokio::try_join!(
        async { Ok(login_manager.receive_block_inhibited_changed().await) },
        login_session.receive_lock(),
        login_session.receive_unlock(),
        login_manager.receive_prepare_for_sleep()
    )?;

    if !ignore_systemd_inhibit {
        if let Ok(block_inhibited) = login_manager.block_inhibited().await {
            handle_block_inhibited(&block_inhibited, &event_sender).await;
        }

        {
            let event_sender = event_sender.clone();
            tokio::spawn(async move {
                while let Some(change) = block_inhibited_stream.next().await {
                    if change.name() == "BlockInhibited" {
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
        tokio::spawn(async move {
            while lock_stream.next().await.is_some() {
                log::info!("Sending SessionLocked(true) event");
                if let Err(e) = event_sender.send(Event::SessionLocked(true)) {
                    log::info!("Failed to get unlock args: {}", e)
                }
            }
        });
    }

    {
        let event_sender = event_sender.clone();
        tokio::spawn(async move {
            while unlock_stream.next().await.is_some() {
                log::info!("Sending SessionLocked(false) event");
                if let Err(e) = event_sender.send(Event::SessionLocked(false)) {
                    log::info!("Failed to get unlock args: {}", e)
                }
            }
        });
    }

    {
        let event_sender = event_sender.clone();
        tokio::spawn(async move {
            while let Some(sleep) = sleep_stream.next().await {
                if let Ok(sleep) = sleep.args() {
                    let start = *sleep.start();
                    log::info!("Sending PrepareForSleep({}) event", start);
                    if let Err(e) = event_sender.send(Event::PrepareForSleep(start)) {
                        log::info!("Failed to get sleep args: {}", e)
                    }
                }
            }
        });
    }

    Ok(())
}
