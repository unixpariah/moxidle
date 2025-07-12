use crate::Event;
use calloop::channel;
use rusb::{Device, Interfaces, UsbContext};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEvent {
    name: String,
    event: String,
}

struct HotPlugHandler(Box<dyn FnMut(DeviceEvent) + 'static + Send>);

impl<T: UsbContext> rusb::Hotplug<T> for HotPlugHandler {
    fn device_arrived(&mut self, device: Device<T>) {
        (self.0)(DeviceEvent {
            name: get_class_name(device.active_config_descriptor().unwrap().interfaces()),
            event: "Added".to_string(),
        });
    }

    fn device_left(&mut self, device: Device<T>) {
        (self.0)(DeviceEvent {
            name: get_class_name(device.config_descriptor(0).unwrap().interfaces()),
            event: "Removed".to_string(),
        });
    }
}

fn get_class_name(interfaces: Interfaces) -> String {
    let mut class_name = String::new();

    for interface in interfaces {
        for descriptor in interface.descriptors() {
            class_name = match descriptor.class_code() {
                1 => "Audio",
                2 => "COMM",
                3 => "HID",
                5 => "Physical",
                6 => "PTP",
                7 => "Printer",
                8 => "MassStorage",
                9 => "Hub",
                10 => "Data",
                _ => "Unknown",
            }
            .to_string();
        }
    }
    class_name
}

pub fn serve(
    event_sender: channel::Sender<Event>,
    usb_context: rusb::Context,
) -> anyhow::Result<()> {
    let registration = rusb::HotplugBuilder::new().enumerate(true).register(
        usb_context,
        Box::new(HotPlugHandler(Box::new(move |_| {
            if let Err(e) = event_sender.send(Event::Usb) {
                log::error!("{e}");
            }
        }))),
    );

    Box::leak(Box::new(registration));

    Ok(())
}
