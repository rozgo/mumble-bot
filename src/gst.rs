use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_audio as gst_audio;
use self::gst::prelude::*;

use byte_slice_cast::*;

use std::i16;
use std::i32;
use std::io::{Error, ErrorKind};
use std::thread;
use std::sync::{Arc, Mutex};

use utils;

use futures;
use futures::{Sink, Stream};
use futures::future::{Future, err, ok, loop_fn, IntoFuture, Loop};


fn sink_pipeline(vox_out_tx: futures::sync::mpsc::Sender<Vec<u8>>) -> Result<gst::Pipeline, utils::ExampleError> {
    gst::init().map_err(utils::ExampleError::InitFailed)?;
    let pipeline = gst::Pipeline::new(None);
    let src = utils::create_element("autoaudiosrc").unwrap();

    let resample = utils::create_element("audioresample").unwrap();

    let caps = gst::Caps::new_simple(
        "audio/x-raw",
        &[
            ("format", &gst_audio::AUDIO_FORMAT_S16.to_string()),
            ("layout", &"interleaved"),
            ("channels", &(1i32)),
            ("rate", &(16000i32)),
        ],
    );

    let caps_filter = utils::create_element("capsfilter").unwrap();
    if let Err(err) = caps_filter.set_property("caps", &caps) {
        panic!("caps_filter.set_property:caps {:?}", err);
    }

    let sink = utils::create_element("appsink")?;

    if let Err(err) = pipeline.add_many(&[&src, &resample, &caps_filter, &sink]) {
        panic!("pipeline.add_many {:?}", err);
    }

    utils::link_elements(&src, &resample).unwrap();
    utils::link_elements(&resample, &caps_filter).unwrap();
    utils::link_elements(&caps_filter, &sink).unwrap();

    let appsink = sink.clone()
        .dynamic_cast::<gst_app::AppSink>()
        .expect("Sink element is expected to be an appsink!");

    appsink.set_caps(&caps);
    let vox_out_tx = Arc::new(Mutex::new(vox_out_tx.wait()));

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
                let mut vox_out_tx = vox_out_tx.lock().unwrap();
                let v = samples.to_vec();
                vox_out_tx.send(v);
                return gst::FlowReturn::Ok;
            } else {
                return gst::FlowReturn::Error;
            };
        },
    ));

    Ok(pipeline)
}

fn sink_loop(pipeline: gst::Pipeline) -> Result<(), utils::ExampleError> {
    
    utils::set_state(&pipeline, gst::State::Playing)?;

    let bus = pipeline
        .get_bus()
        .expect("Pipeline without bus. Shouldn't happen!");

    println!("start sink_loop");

    while let Some(msg) = bus.timed_pop(gst::CLOCK_TIME_NONE) {
        use self::gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => break,
            MessageView::Error(err) => {
                utils::set_state(&pipeline, gst::State::Null)?;
                return Err(utils::ExampleError::ElementError(
                    msg.get_src().get_path_string(),
                    err.get_error(),
                    err.get_debug().unwrap(),
                ));
            }
            _ => (),
        }
    }

    println!("stop sink_loop");

    utils::set_state(&pipeline, gst::State::Null)?;

    Ok(())
}

pub fn sink_main(vox_out_tx: futures::sync::mpsc::Sender<Vec<u8>>) -> impl Fn() -> () {
    let pipeline = sink_pipeline(vox_out_tx).unwrap();
    let p = pipeline.clone();

    thread::spawn(move || {
        sink_loop(pipeline);
    });

    move|| {
        utils::set_state(&p, gst::State::Null);
    }
}

fn src_pipeline() -> Result<(gst::Pipeline, Vec<gst_app::AppSrc>), utils::ExampleError> {
    gst::init().map_err(utils::ExampleError::InitFailed)?;

    let caps = gst::Caps::new_simple(
        "audio/x-raw",
        &[
            ("format", &gst_audio::AUDIO_FORMAT_S16.to_string()),
            ("layout", &"interleaved"),
            ("channels", &(1i32)),
            ("rate", &(16000i32)),
        ],
    );

    let pipeline = gst::Pipeline::new(None);

    let mut appsrcs = Vec::new();
    for _ in 0..16 {
        let appsrc = utils::create_element("appsrc")?;
        let audioconvert = utils::create_element("audioconvert")?;
        let sink = utils::create_element("autoaudiosink")?;
        sink.set_property("async-handling", &true).expect("Unable to set property in the element");
        pipeline.add_many(&[&appsrc, &audioconvert, &sink]).expect("Unable to add elements in the pipeline");
        utils::link_elements(&appsrc, &audioconvert)?;
        utils::link_elements(&audioconvert, &sink)?;
        appsrcs.push(Box::new(appsrc));
    }

    let appsrcs = appsrcs.iter().map(|src| {
        let appsrc = (*src).clone()
            .dynamic_cast::<gst_app::AppSrc>()
            .expect("Source element is expected to be an appsrc!");
        appsrc.set_caps(&caps);
        appsrc
    });

    let appsrcs = appsrcs.collect();

    pipeline.use_clock(None::<&gst::Clock>);

    Ok((pipeline, appsrcs))
}

fn src_rx<'a>(appsrc: Vec<gst_app::AppSrc>, vox_inp_rx: futures::sync::mpsc::Receiver<(i32, Vec<u8>)>)
-> impl Future<Item = (), Error = Error> + 'a {
    vox_inp_rx.fold(appsrc, |appsrc, (session, bytes)| {
        if session >= 0 {
            let idx = session as usize;
            let buffer = gst::Buffer::from_slice(bytes).expect("gst::Buffer::from_slice(bytes)");
            //buffer.set_pts(i * 500 * gst::MSECOND);
            if appsrc[idx].push_buffer(buffer) != gst::FlowReturn::Ok {
                appsrc[idx].end_of_stream();
                err(())
            }
            else {
                ok(appsrc)
            }
        }
        else {
            ok(appsrc)
        }
    })
    .map(|_| ())
    .map_err(|_| Error::new(ErrorKind::Other, "vox_inp_task"))
}

fn src_loop(pipeline: gst::Pipeline) -> Result<(), utils::ExampleError> {

    utils::set_state(&pipeline, gst::State::Playing)?;

    let bus = pipeline
        .get_bus()
        .expect("Pipeline without bus. Shouldn't happen!");

    println!("start src_loop");

    while let Some(msg) = bus.timed_pop(gst::CLOCK_TIME_NONE) {
        use self::gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => break,
            MessageView::Error(err) => {
                utils::set_state(&pipeline, gst::State::Null)?;
                return Err(utils::ExampleError::ElementError(
                    msg.get_src().get_path_string(),
                    err.get_error(),
                    err.get_debug().unwrap(),
                ));
            }
            _ => (),
        }
    }

    utils::set_state(&pipeline, gst::State::Null)?;

    println!("stop src_loop");

    Ok(())
}

pub fn src_main<'a>(vox_inp_rx: futures::sync::mpsc::Receiver<(i32, Vec<u8>)>)
-> (impl Fn() -> (), impl Future<Item = (), Error = Error> + 'a) {
    let (pipeline, appsrcs) = src_pipeline().unwrap();
    let p = pipeline.clone();

    thread::spawn(move || {
        println!("start thread src_loop");
        src_loop(pipeline).unwrap();
        println!("stop thread src_loop");
    });

    let kill_pipe = move|| {
        utils::set_state(&p, gst::State::Null);
    };

    (kill_pipe, src_rx(appsrcs, vox_inp_rx))
}
