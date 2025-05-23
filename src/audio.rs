use crate::Event;
use calloop::channel;
use libpulse_binding::{
    self as pulse,
    callbacks::ListResult,
    context::{subscribe::InterestMaskSet, FlagSet},
    error::{Code, PAErr},
    mainloop::threaded::Mainloop,
};
use pulse::context::Context;
use std::{cell::Cell, rc::Rc};

fn process_sink_inputs(
    introspector: &pulse::context::introspect::Introspector,
    event_sender: &channel::Sender<Event>,
) {
    let playing = Rc::new(Cell::new(false));
    introspector.get_sink_input_info_list({
        let playing = playing.clone();
        let event_sender = event_sender.clone();
        move |result| match result {
            ListResult::Error => {
                log::error!("Error retrieving sink input info list")
            }
            ListResult::Item(info) => {
                if !info.corked {
                    playing.set(true);
                    if let Some(app_name) = info
                        .proplist
                        .get_str(pulse::proplist::properties::APPLICATION_NAME)
                    {
                        log::info!("Audio playing by: {}", app_name);
                    }
                }
            }
            ListResult::End => {
                let is_playing = playing.get();
                log::info!("Sending AudioInhibit({}) event", is_playing);
                if let Err(e) = event_sender.send(Event::AudioInhibit(is_playing)) {
                    log::error!("Failed to send AudioInhibit({}) event: {}", is_playing, e);
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
                return Err(PAErr(Code::ConnectionTerminated as i32))
            }
            _ => mainloop.wait(),
        }
    }

    let introspector = context.introspect();

    process_sink_inputs(&introspector, &event_sender);
    context.set_subscribe_callback(Some(Box::new({
        move |_, _, _| {
            process_sink_inputs(&introspector, &event_sender);
        }
    })));
    context.subscribe(InterestMaskSet::SINK_INPUT, |_| {});

    // PulseAudio's event loop (mainloop) and context must remain alive
    // for the duration of the subscription.
    Box::leak(Box::new(context));
    Box::leak(Box::new(mainloop));

    Ok(())
}
