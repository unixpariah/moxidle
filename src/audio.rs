use crate::Event;
use calloop::channel;
use libpulse_binding::{
    self as pulse,
    callbacks::ListResult,
    context::{FlagSet, subscribe::InterestMaskSet},
    error::{Code, PAErr},
    mainloop::threaded::Mainloop,
    proplist,
};
use pulse::context::Context;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Debug)]
struct AudioInhibitor {
    app_name: String,
    pid: String,
    binary: String,
    media_name: Option<String>,
    media_title: Option<String>,
}

impl AudioInhibitor {
    fn new(proplist: &proplist::Proplist) -> Option<Self> {
        let app_name = proplist.get_str(pulse::proplist::properties::APPLICATION_NAME)?;
        let binary = proplist.get_str(pulse::proplist::properties::APPLICATION_PROCESS_BINARY)?;
        let pid = proplist.get_str(pulse::proplist::properties::APPLICATION_PROCESS_ID)?;
        let media_name = proplist.get_str(pulse::proplist::properties::MEDIA_NAME);
        let media_title = proplist.get_str(pulse::proplist::properties::MEDIA_TITLE);

        Some(Self {
            app_name,
            pid,
            binary,
            media_name,
            media_title,
        })
    }
}

impl std::fmt::Display for AudioInhibitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "application '{}' (PID: {}, Binary: {}, Media: {}, Title: {})",
            self.app_name,
            self.pid,
            self.binary,
            self.media_name.as_deref().unwrap_or("unknown"),
            self.media_title.as_deref().unwrap_or("unknown")
        )
    }
}

fn process_sink_inputs(
    inhibitors: Arc<Mutex<HashMap<String, AudioInhibitor>>>,
    introspector: &pulse::context::introspect::Introspector,
    event_sender: &channel::Sender<Event>,
) {
    introspector.get_sink_input_info_list({
        let event_sender = event_sender.clone();
        move |result| match result {
            ListResult::Error => {
                log::error!("Error retrieving sink input info list")
            }
            ListResult::Item(info) => {
                let mut inhibitors = inhibitors.lock().unwrap();

                if !info.corked {
                    if let Some(inhibitor) = AudioInhibitor::new(&info.proplist) {
                        log::info!("Added audio inhibitor for {inhibitor}");
                        inhibitors.insert(inhibitor.binary.clone(), inhibitor);
                    }
                } else if let Some(name) = info
                    .proplist
                    .get_str(pulse::proplist::properties::APPLICATION_PROCESS_BINARY)
                    && let Some(removed) = inhibitors.remove(&name)
                {
                    log::info!("Removed audio inhibitor for {removed}");
                }
            }
            ListResult::End => {
                if let Err(e) =
                    event_sender.send(Event::AudioInhibit(!inhibitors.lock().unwrap().is_empty()))
                {
                    log::error!("Failed to send AudioInhibit event: {e}");
                }
            }
        }
    });
}

pub async fn serve(
    event_sender: channel::Sender<Event>,
    ignore_audio_inhibit: bool,
) -> Result<(), pulse::error::PAErr> {
    if ignore_audio_inhibit {
        return Ok(());
    }

    let inhibitors = Arc::new(Mutex::new(HashMap::new()));

    let mut mainloop = Mainloop::new().ok_or(PAErr(Code::NoData as i32))?;
    let mut context =
        Context::new(&mainloop, "playback-listener").ok_or(PAErr(Code::NoData as i32))?;
    context.connect(None, FlagSet::NOFLAGS, None)?;
    mainloop.start()?;

    loop {
        match context.get_state() {
            pulse::context::State::Ready => break,
            pulse::context::State::Failed => return Err(PAErr(Code::ConnectionRefused as i32)),
            pulse::context::State::Terminated => {
                return Err(PAErr(Code::ConnectionTerminated as i32));
            }
            _ => mainloop.wait(),
        }
    }

    let introspector = context.introspect();

    process_sink_inputs(Arc::clone(&inhibitors), &introspector, &event_sender);
    context.set_subscribe_callback(Some(Box::new({
        let inhibitors = Arc::clone(&inhibitors);
        move |_, _, _| {
            process_sink_inputs(Arc::clone(&inhibitors), &introspector, &event_sender);
        }
    })));
    context.subscribe(InterestMaskSet::SINK_INPUT, |_| {});

    // PulseAudio's event loop (mainloop) and context must remain alive
    // for the duration of the subscription.
    Box::leak(Box::new(context));
    Box::leak(Box::new(mainloop));

    Ok(())
}
