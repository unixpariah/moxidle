use crate::Event;
use calloop::channel;
use futures_lite::StreamExt;
use zbus::proxy;

#[derive(Default)]
pub struct Power {
    pub unplugged: bool,
    pub percentage: f64,
}

#[proxy(interface = "org.freedesktop.UPower", assume_defaults = true)]
trait UPower {
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;

    #[zbus(object = "Device")]
    fn get_display_device(&self);
}

#[proxy(
    interface = "org.freedesktop.UPower.Device",
    default_service = "org.freedesktop.UPower",
    assume_defaults = false
)]
trait Device {
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
}

pub async fn serve(
    event_sender: channel::Sender<Event>,
    ignore_on_battery: bool,
    ignore_battery_percentage: bool,
) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;

    let upower = UPowerProxy::new(&connection).await?;

    let mut on_battery_stream = upower.receive_on_battery_changed().await;

    if !ignore_on_battery {
        let event_sender = event_sender.clone();
        tokio::spawn(async move {
            while let Some(event) = on_battery_stream.next().await {
                if let Ok(on_battery) = event.get().await {
                    if let Err(e) = event_sender.send(Event::OnBattery(on_battery)) {
                        log::info!("Failed to get OnBattery args: {}", e)
                    }
                }
            }
        });
    }

    if !ignore_battery_percentage {
        let device = upower.get_display_device().await?;
        let percentage = device.percentage().await?;
        if let Err(e) = event_sender.send(Event::BatteryPercentage(percentage)) {
            log::info!("Failed to get OnBattery args: {}", e)
        }

        let mut percentage_stream = device.receive_percentage_changed().await;

        let event_sender = event_sender.clone();
        tokio::spawn(async move {
            while let Some(event) = percentage_stream.next().await {
                if let Ok(percentage) = event.get().await {
                    if let Err(e) = event_sender.send(Event::BatteryPercentage(percentage)) {
                        log::info!("Failed to get BatteryPercentage args: {}", e)
                    }
                }
            }
        });
    }

    Ok(())
}
