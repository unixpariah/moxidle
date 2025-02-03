use crate::Event;
use calloop::channel;
use futures_lite::StreamExt;
use std::sync::Arc;
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

fn handle_battery_percentage(event_sender: &channel::Sender<Event>, value: f64) {
    log::info!("Sending BatteryPercentage({}) event", value);
    if let Err(e) = event_sender.send(Event::BatteryPercentage(value)) {
        log::info!("Failed to get OnBattery args: {}", e)
    }
}

fn handle_on_battery(event_sender: &channel::Sender<Event>, value: bool) {
    log::info!("Sending OnBattery({}) event", value);
    if let Err(e) = event_sender.send(Event::OnBattery(value)) {
        log::info!("Failed to get OnBattery args: {}", e)
    }
}

pub async fn serve(
    connection: Arc<zbus::Connection>,
    event_sender: channel::Sender<Event>,
    ignore_on_battery: bool,
    ignore_battery_percentage: bool,
) -> zbus::Result<()> {
    let upower = UPowerProxy::new(&connection).await?;

    if !ignore_on_battery {
        let mut on_battery_stream = upower.receive_on_battery_changed().await;
        let event_sender = event_sender.clone();
        if let Ok(on_battery) = upower.on_battery().await {
            handle_on_battery(&event_sender, on_battery);
        }

        tokio::spawn(async move {
            while let Some(event) = on_battery_stream.next().await {
                if let Ok(on_battery) = event.get().await {
                    handle_on_battery(&event_sender, on_battery);
                }
            }
        });
    }

    if !ignore_battery_percentage {
        let device = upower.get_display_device().await?;
        let percentage = device.percentage().await?;
        handle_battery_percentage(&event_sender, percentage);

        let mut percentage_stream = device.receive_percentage_changed().await;

        let event_sender = event_sender.clone();
        tokio::spawn(async move {
            while let Some(event) = percentage_stream.next().await {
                if let Ok(percentage) = event.get().await {
                    handle_battery_percentage(&event_sender, percentage);
                }
            }
        });
    }

    Ok(())
}
