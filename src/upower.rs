use crate::Event;
use calloop::channel;
use futures_lite::StreamExt;
use zbus::proxy;

#[proxy(interface = "org.freedesktop.UPower", assume_defaults = true)]
trait UPower {
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
}

pub async fn serve(sender: channel::Sender<Event>) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let upower = UPowerProxy::new(&connection).await?;
    let mut stream = upower.receive_on_battery_changed().await;
    while let Some(event) = stream.next().await {
        if let Err(e) = sender.send(Event::OnBattery(event.get().await?)) {
            log::info!("Failed to get OnBattery args: {}", e)
        }
    }
    Ok(())
}
