use crate::Event;
use calloop::channel;
use libpulse_binding::{
    self as pulse,
    callbacks::ListResult,
    context::{subscribe::InterestMaskSet, FlagSet},
    error::PAErr,
    mainloop::threaded::Mainloop,
};
use pulse::context::Context;
use std::{cell::Cell, rc::Rc};

pub async fn serve(
    event_sender: channel::Sender<Event>,
    ignore_audio_inhibit: bool,
) -> Result<(), pulse::error::PAErr> {
    if ignore_audio_inhibit {
        return Ok(());
    }

    let mut mainloop = Mainloop::new().ok_or(PAErr(0))?;
    let mut context = Context::new(&mainloop, "volume-change-listener").ok_or(PAErr(0))?;
    context.connect(None, FlagSet::NOFLAGS, None)?;
    mainloop.start()?;

    while pulse::context::State::Ready != context.get_state() {}

    let introspector = context.introspect();
    context.set_subscribe_callback(Some(Box::new(move |_, _, _| {
        let playing = Rc::new(Cell::new(false));
        introspector.get_sink_input_info_list({
            let event_sender = event_sender.clone();
            let playing = Rc::clone(&playing);
            move |list| match list {
                ListResult::Error => log::error!("Error while retrieving sink input info list"),
                ListResult::End => {
                    if let Err(e) = event_sender.send(Event::AudioInhibit(playing.get())) {
                        log::error!("Failed to send AudioInhibit event {}", e);
                    }
                }
                ListResult::Item(item) => {
                    if !item.corked {
                        playing.set(true);
                    }
                }
            }
        });
    })));
    context.subscribe(InterestMaskSet::SINK_INPUT, |_| {});

    // PulseAudio's event loop (mainloop) and context must remain alive
    // for the duration of the subscription.
    let mainloop = Box::new(mainloop);
    let context = Box::new(context);
    Box::leak(context);
    Box::leak(mainloop);

    Ok(())
}
