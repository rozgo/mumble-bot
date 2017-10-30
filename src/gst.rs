use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_audio as gst_audio;
use self::gst::prelude::*;

use byte_slice_cast::*;

use std::i16;
use std::i32;
use std::io::{Error, ErrorKind};
use std::thread;

use utils;

use futures;
use futures::{Sink, Stream};
use futures::future::{Future, err, ok, loop_fn, IntoFuture, Loop};

fn sink_pipeline(vox_out_tx: futures::sync::mpsc::Sender<Vec<u8>>) -> Result<gst::Pipeline, utils::ExampleError> {
    gst::init().map_err(utils::ExampleError::InitFailed)?;
    let pipeline = gst::Pipeline::new(None);
    let src = utils::create_element("autoaudiosrc")?;

    let resample = utils::create_element("audioresample")?;

    let caps = gst::Caps::new_simple(
        "audio/x-raw",
        &[
            ("format", &gst_audio::AUDIO_FORMAT_S16.to_string()),
            ("layout", &"interleaved"),
            ("channels", &(1i32)),
            ("rate", &(16000i32)),
        ],
    );

    let caps_filter = utils::create_element("capsfilter")?;
    if let Err(err) = caps_filter.set_property("caps", &caps) {
        panic!("caps_filter.set_property:caps {:?}", err);
    }

    let sink = utils::create_element("appsink")?;

    if let Err(err) = pipeline.add_many(&[&src, &resample, &caps_filter, &sink]) {
        panic!("pipeline.add_many {:?}", err);
    }

    utils::link_elements(&src, &resample)?;
    utils::link_elements(&resample, &caps_filter)?;
    utils::link_elements(&caps_filter, &sink)?;

    let appsink = sink.clone()
        .dynamic_cast::<gst_app::AppSink>()
        .expect("Sink element is expected to be an appsink!");

    appsink.set_caps(&caps);

    appsink.set_callbacks(gst_app::AppSinkCallbacks::new(
        /* eos */
        |_| {},
        /* new_preroll */
        |_| gst::FlowReturn::Ok,
        /* new_samples */
        move |appsink| {
            let sample = match appsink.pull_sample() {
                None => return gst::FlowReturn::Eos,
                Some(sample) => sample,
            };

            let buffer = sample
                .get_buffer()
                .expect("Unable to extract buffer from the sample");

            let map = buffer
                .map_readable()
                .expect("Unable to map buffer for reading");

            if let Ok(samples) = map.as_slice().as_slice_of::<u8>() {
                    let vox_out_tx = vox_out_tx.clone();
                    let v = samples.to_vec();
                    let tx0 = vox_out_tx.send(v);
                    tx0.wait().unwrap();
                    return gst::FlowReturn::Ok;
            } else {
                return gst::FlowReturn::Error;
            };
        },
    ));

    Ok(pipeline)
}

fn set_state(e: &gst::Pipeline, state: gst::State) -> Result<(), Error> {
    if let gst::StateChangeReturn::Failure = e.set_state(state) {
        return Err(Error::new(ErrorKind::Other,
            gst::Element::state_get_name(state).unwrap()
        ));
    }
    Ok(())
}

fn sink_loop(vox_out_tx: futures::sync::mpsc::Sender<Vec<u8>>) -> Result<(), Error> {
    let pipeline = sink_pipeline(vox_out_tx).unwrap();

    set_state(&pipeline, gst::State::Playing)?;

    let bus = pipeline
        .get_bus()
        .expect("Pipeline without bus. Shouldn't happen!");

    while let Some(msg) = bus.timed_pop(gst::CLOCK_TIME_NONE) {
        use self::gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => break,
            MessageView::Error(err) => {
                set_state(&pipeline, gst::State::Null)?;
                return Err(Error::new(ErrorKind::Other,
                    format!("Error from {}: {} ({:?})",
                    msg.get_src().get_path_string(),
                    err.get_error(),
                    err.get_debug().unwrap()
                )));
            }
            _ => (),
        }
    }

    set_state(&pipeline, gst::State::Null)?;

    Ok(())
}

pub fn sink_main(vox_out_tx: futures::sync::mpsc::Sender<Vec<u8>>) {
    thread::spawn(move || {
        sink_loop(vox_out_tx.clone()).unwrap();
    });
}

fn src_pipeline() -> Result<(gst::Pipeline, gst_app::AppSrc), utils::ExampleError> {
    gst::init().map_err(utils::ExampleError::InitFailed)?;

    let pipeline = gst::Pipeline::new(None);
    let src = utils::create_element("appsrc")?;
    let audioconvert = utils::create_element("audioconvert")?;
    let sink = utils::create_element("autoaudiosink")?;

    pipeline
        .add_many(&[&src, &audioconvert, &sink])
        .expect("Unable to add elements in the pipeline");
    utils::link_elements(&src, &audioconvert)?;
    utils::link_elements(&audioconvert, &sink)?;

    let appsrc = src.clone()
        .dynamic_cast::<gst_app::AppSrc>()
        .expect("Source element is expected to be an appsrc!");

    let caps = gst::Caps::new_simple(
        "audio/x-raw",
        &[
            ("format", &gst_audio::AUDIO_FORMAT_S16.to_string()),
            ("layout", &"interleaved"),
            ("channels", &(1i32)),
            ("rate", &(16000i32)),
        ],
    );

    appsrc.set_caps(&caps);

    Ok((pipeline, appsrc))
}

fn src_rx<'a>(appsrc: gst_app::AppSrc, vox_inp_rx: futures::sync::mpsc::Receiver<Vec<u8>>)
-> impl Future<Item = (), Error = Error> + 'a {
    vox_inp_rx.fold((), move |_, bytes| {
        let buffer = gst::Buffer::from_slice(bytes).unwrap();
        //buffer.set_pts(i * 500 * gst::MSECOND);
        if appsrc.push_buffer(buffer) != gst::FlowReturn::Ok {
            appsrc.end_of_stream();
            err(())
        }
        else {
            ok(())
        }
    })
    .map_err(|_| Error::new(ErrorKind::Other, "vox_inp_task"))
}

fn src_loop(pipeline: gst::Pipeline) -> Result<(), Error> {

    set_state(&pipeline, gst::State::Playing)?;

    let bus = pipeline
        .get_bus()
        .expect("Pipeline without bus. Shouldn't happen!");

    while let Some(msg) = bus.timed_pop(gst::CLOCK_TIME_NONE) {
        use self::gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => break,
            MessageView::Error(err) => {
                set_state(&pipeline, gst::State::Null)?;
                return Err(Error::new(ErrorKind::Other,
                    format!("Error from {}: {} ({:?})",
                    msg.get_src().get_path_string(),
                    err.get_error(),
                    err.get_debug().unwrap()
                )));
            }
            _ => (),
        }
    }

    set_state(&pipeline, gst::State::Null)?;

    Ok(())
}

pub fn src_main<'a>(vox_inp_rx: futures::sync::mpsc::Receiver<Vec<u8>>)
-> impl Future<Item = (), Error = Error> + 'a {
    let (pipeline, appsrc) = src_pipeline().unwrap();

    thread::spawn(move || {
        src_loop(pipeline).unwrap();
    });

    src_rx(appsrc, vox_inp_rx)
}
