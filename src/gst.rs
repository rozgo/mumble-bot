// extern crate gstreamer as gst;
// use gst::prelude::*;
// use gst::BinExtManual;
// use gst::byte_slice_cast::AsSliceOf;

extern crate gstreamer as gst;
use self::gst::prelude::*;

extern crate gstreamer_app as gst_app;
extern crate gstreamer_audio as gst_audio;

extern crate glib;

extern crate byte_slice_cast;
use self::byte_slice_cast::*;

use std::i16;
use std::i32;

use utils;

extern crate futures;
use self::futures::{Sink, Stream};
use self::futures::future::{Future, ok, loop_fn, IntoFuture, Loop};

fn create_pipeline(vox_out_tx: futures::sync::mpsc::Sender<Vec<u8>>) -> Result<gst::Pipeline, utils::ExampleError> {
    gst::init().map_err(utils::ExampleError::InitFailed)?;
    let pipeline = gst::Pipeline::new(None);
    let uri : &str = "/Users/rozgo/Projects/mumble-bot/data/man16kHz.raw";

    // let src = utils::create_element("audiotestsrc")?;
    // src.set_property("wave", 2).unwrap();
    // src.set_property("freq", &(200.0)).unwrap();

    // let src = utils::create_element("filesrc")?;
    let src = utils::create_element("osxaudiosrc")?;
    // let src = gst::ElementFactory::make("osxaudiosrc", None).unwrap();

    // let src = utils::create_element("filesrc")?;
    // src.set_property("location", &glib::Value::from(uri)).unwrap();

    let resample = utils::create_element("audioresample")?;

    let caps = gst::Caps::new_simple(
        "audio/x-raw",
        &[
            ("format", &gst_audio::AUDIO_FORMAT_S16.to_string()),
            ("layout", &"interleaved"),
            ("channels", &(1i32)),
            // ("rate", &gst::IntRange::<i32>::new(1, i32::MAX)),
            ("rate", &(16000i32)),
        ],
    );

    let caps_filter = utils::create_element("capsfilter")?;

    // let caps = caps_filter.clone()
    //     .dynamic_cast::<gst_app::AppSink>()
    //     .expect("Sink element is expected to be an appsink!");
    // caps_filter.set_caps(&gst::Caps::new_simple(
    //     "audio/x-raw",
    //     &[
    //         ("format", &gst_audio::AUDIO_FORMAT_S16.to_string()),
    //         ("layout", &"interleaved"),s
    //         ("channels", &(1i32)),
    //         // ("rate", &gst::IntRange::<i32>::new(1, i32::MAX)),
    //         ("rate", &(16000i32)),
    //     ],
    // ));
    if let Err(err) = caps_filter.set_property("caps", &caps) {
        println!("caps_filter.set_property:caps {:?}", err);
    }

    let sink = utils::create_element("appsink")?;

    if let Err(err) = pipeline.add_many(&[&src, &resample, &caps_filter, &sink]) {
        println!("pipeline.add_many {:?}", err);
    }

    // utils::link_elements(&src, &sink)?;

    if let Err(err) = gst::Element::link_many(&[&src, &resample, &caps_filter, &sink]) {
        println!("gst::Element::link_many {:?}", err);
    }

    let appsink = sink.clone()
        .dynamic_cast::<gst_app::AppSink>()
        .expect("Sink element is expected to be an appsink!");

    println!("appsink {:?}", appsink);

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
                    // samples
                    return gst::FlowReturn::Ok;
            } else {
                return gst::FlowReturn::Error;
            };

            // let sum: f64 = samples
            //     .iter()
            //     .map(|sample| {
            //         let f = f64::from(*sample) / f64::from(i16::MAX);
            //         f * f
            //     })
            //     .sum();
            // let rms = (sum / (samples.len() as f64)).sqrt();
            // println!("rms: {}", rms);

            
        },
    ));

    println!("Ok(pipeline)");

    Ok(pipeline)
}

fn main_loop(vox_out_tx: futures::sync::mpsc::Sender<Vec<u8>>) -> Result<(), utils::ExampleError> {
    let pipeline = create_pipeline(vox_out_tx)?;

    utils::set_state(&pipeline, gst::State::Playing)?;

    let bus = pipeline
        .get_bus()
        .expect("Pipeline without bus. Shouldn't happen!");

    while let Some(msg) = bus.timed_pop(gst::CLOCK_TIME_NONE) {
        use self::gst::MessageView;

        match msg.view() {
            MessageView::Eos(..) => break,
            MessageView::Error(err) => {
                // println!("Error! {:?}", err.to_str());
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

    Ok(())
}

pub fn gst_main(vox_out_tx: futures::sync::mpsc::Sender<Vec<u8>>) {
    match main_loop(vox_out_tx) {
        Ok(r) => r,
        Err(e) => println!("Error! {}", e),
    }
}