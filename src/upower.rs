use crate::Event;
use calloop::channel;
use futures_lite::StreamExt;
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::{fmt::Display, sync::Arc};
use zbus::{proxy, zvariant::OwnedValue};

#[derive(PartialEq, OwnedValue, Deserialize_repr, Serialize_repr, Default, Debug)]
#[repr(u32)]
pub enum BatteryState {
    #[default]
    Unknown = 0,
    Charging = 1,
    Discharging = 2,
    Empty = 3,
    FullyCharged = 4,
    PendingCharge = 5,
    PendingDischarge = 6,
}

impl Display for BatteryState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BatteryState::Unknown => "unknown",
            BatteryState::Charging => "charging",
            BatteryState::Discharging => "discharging",
            BatteryState::Empty => "empty",
            BatteryState::FullyCharged => "fullycharged",
            BatteryState::PendingCharge => "pendingcharge",
            BatteryState::PendingDischarge => "pendingdischarge",
        };
        write!(f, "{s}")
    }
}

#[derive(PartialEq, OwnedValue, Deserialize_repr, Serialize_repr, Default, Debug)]
#[repr(u32)]
pub enum BatteryLevel {
    #[default]
    Unknown = 0,
    None = 1,
    Low = 3,
    Critical = 4,
    Normal = 6,
    High = 7,
    Full = 8,
}

impl Display for BatteryLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BatteryLevel::Unknown => "unknown",
            BatteryLevel::None => "none",
            BatteryLevel::Low => "low",
            BatteryLevel::Critical => "critical",
            BatteryLevel::Normal => "normal",
            BatteryLevel::High => "high",
            BatteryLevel::Full => "full",
        };

        write!(f, "{s}")
    }
}

#[derive(Default, PartialEq)]
pub enum PowerSource {
    #[default]
    Battery,
    Plugged,
}

#[derive(Default)]
pub struct Power {
    source: PowerSource,
    level: BatteryLevel,
    state: BatteryState,
    percentage: f64,
}

#[derive(PartialEq)]
pub enum LevelComparison {
    Below,
    Above,
    Equal,
}

impl Power {
    pub fn source(&self) -> &PowerSource {
        &self.source
    }

    pub fn percentage(&self) -> f64 {
        self.percentage
    }

    pub fn update_level(&mut self, level: BatteryLevel) {
        self.level = level;
    }

    pub fn update_state(&mut self, state: BatteryState) {
        self.state = state;
    }

    pub fn update_source(&mut self, on_battery: bool) {
        self.source = if on_battery {
            PowerSource::Battery
        } else {
            PowerSource::Plugged
        };
    }

    pub fn update_percentage(&mut self, new_percentage: f64) {
        self.percentage = new_percentage.clamp(0.0, 100.0);
    }

    pub fn level_cmp(&self, threshold: &f64) -> LevelComparison {
        match self.percentage() {
            power if power.lt(threshold) => LevelComparison::Below,
            power if power.gt(threshold) => LevelComparison::Above,
            power if power.eq(threshold) => LevelComparison::Equal,
            _ => unreachable!(),
        }
    }

    pub fn state(&self) -> &BatteryState {
        &self.state
    }

    pub fn level(&self) -> &BatteryLevel {
        &self.level
    }
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

    #[zbus(property)]
    fn battery_level(&self) -> zbus::Result<BatteryLevel>;

    #[zbus(property)]
    fn state(&self) -> zbus::Result<BatteryState>;
}

fn handle_battery_percentage(event_sender: &channel::Sender<Event>, value: f64) {
    if let Err(e) = event_sender.send(Event::BatteryPercentage(value)) {
        log::warn!("Failed to get BatteryPercentage args: {e}")
    }
}

fn handle_state(event_sender: &channel::Sender<Event>, value: BatteryState) {
    if let Err(e) = event_sender.send(Event::BatteryState(value)) {
        log::warn!("Failed to send BatteryState event: {e}")
    }
}

fn handle_battery_level(event_sender: &channel::Sender<Event>, value: BatteryLevel) {
    if let Err(e) = event_sender.send(Event::BatteryLevel(value)) {
        log::warn!("Failed to send BatteryLevel event: {e}")
    }
}

fn handle_on_battery(event_sender: &channel::Sender<Event>, value: bool) {
    if let Err(e) = event_sender.send(Event::OnBattery(value)) {
        log::warn!("Failed to send OnBattery event: {e}")
    }
}

pub async fn serve(
    connection: Arc<zbus::Connection>,
    event_sender: channel::Sender<Event>,
    ignore_on_battery: bool,
    ignore_battery_percentage: bool,
    ignore_battery_state: bool,
    ignore_battery_level: bool,
) -> zbus::Result<()> {
    if ignore_on_battery
        && ignore_battery_percentage
        && ignore_battery_state
        && ignore_battery_level
    {
        return Ok(());
    }

    let upower = UPowerProxy::new(&connection).await?;

    if !ignore_on_battery {
        let mut on_battery_stream = upower.receive_on_battery_changed().await;
        log::info!("OnBattery listener active");
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

    if ignore_battery_percentage && ignore_battery_state && ignore_battery_level {
        return Ok(());
    }

    let upower_clone = upower.clone();
    let event_sender_clone = event_sender.clone();
    tokio::spawn(async move {
        let device = match upower_clone.get_display_device().await {
            Ok(device) => device,
            Err(e) => {
                log::error!("Failed to get display device: {e}");
                return;
            }
        };

        if !ignore_battery_percentage {
            let mut percentage_stream = device.receive_percentage_changed().await;
            log::info!("BatteryPercentage listener active");

            let event_sender = event_sender_clone.clone();
            tokio::spawn(async move {
                while let Some(event) = percentage_stream.next().await {
                    if let Ok(percentage) = event.get().await {
                        handle_battery_percentage(&event_sender, percentage);
                    }
                }
            });
        }

        if !ignore_battery_state {
            if let Ok(state) = device.state().await {
                handle_state(&event_sender_clone, state);
            }

            let mut state_stream = device.receive_state_changed().await;
            log::info!("BatteryState listener active");

            let event_sender = event_sender_clone.clone();
            tokio::spawn(async move {
                while let Some(event) = state_stream.next().await {
                    if let Ok(state) = event.get().await {
                        handle_state(&event_sender, state);
                    }
                }
            });
        }

        if !ignore_battery_level {
            if let Ok(level) = device.battery_level().await {
                handle_battery_level(&event_sender_clone, level);
            }

            let mut level_stream = device.receive_battery_level_changed().await;
            log::info!("BatteryLevel listener active");

            let event_sender = event_sender_clone.clone();
            tokio::spawn(async move {
                while let Some(event) = level_stream.next().await {
                    if let Ok(level) = event.get().await {
                        handle_battery_level(&event_sender, level);
                    }
                }
            });
        }
    });

    Ok(())
}
