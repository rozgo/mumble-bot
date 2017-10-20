use opus;
use std::io::{Cursor, Write, Read, Error, ErrorKind};
use byteorder::{BigEndian, LittleEndian, ReadBytesExt, WriteBytesExt};

use tokio_io;
use tokio_io::AsyncRead;
use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;

use futures;
use futures::future::{ok, loop_fn, Loop};
use futures::stream::Stream;
use futures::{Sink, Poll, Future, Async};

use chrono;

use mumble;

use varint;
use varint::VarintReader;
use varint::VarintWriter;

use protobuf::Message;
use protobuf::{CodedOutputStream, CodedInputStream};

use session;


pub fn opus_decode(decoder: &mut opus::Decoder, opus_frame: Vec<u8>) -> (Vec<u8>, bool) {

    println!("!!__@__!!__@__!!__@__!!__@__!!__@__!!__@__!!__@__!!__@__!!__@__!!__@__");

    let mut rdr = Cursor::new(opus_frame);

    let mut opus_frame = Vec::<u8>::new();
    let mut segments = 0;

    let mut opus_done = false;
    while let Ok(opus_header) = rdr.read_varint() {
        opus_done = opus_header & 0x2000 == 0x2000;
        let opus_length = opus_header & 0x1FFF;
        println!("opus length: {} done: {}", opus_length, opus_done);
        let mut segment = vec![0u8; opus_length as usize];
        match rdr.read_exact(&mut segment[..]) {
            Ok(()) => opus_frame.write_all(&segment).unwrap(),
            Err(err) => println!("{}", err),
        };
        
        println!("opus size: {} segment: {}", opus_length, segments);
        segments = segments + 1;
    }

    // util::opus_analyze(&opus_frame);

    let mut sample_pcm = vec![0i16; 320 * segments];

    let size: usize =
        match decoder.decode(&opus_frame[..], &mut sample_pcm[..], false) {
            Ok(size) => size,
            Err(err) => {
                println!("{}", err); 0},
        };

    println!("pcm size: {}", size);

    let mut pcm_data = Vec::<u8>::new();
    for s in 0..size {
        pcm_data.write_i16::<LittleEndian>(sample_pcm[s]).unwrap();
    }

    (pcm_data, opus_done)
}

pub fn opus_analyze(opus_data : &Vec<u8>) {
    println!("======================================================");
    if let Ok(_) = opus::packet::parse(&opus_data) {
        println!("opus::packet::parse: ok");
    }
    if let Ok(p) = opus::packet::get_nb_frames(&opus_data) {
        println!("opus::packet::get_nb_frames: {:?}", p);
    }
    if let Ok(p) = opus::packet::get_nb_samples(&opus_data, 48000) {
        println!("opus::packet::get_nb_samples: {:?}", p);
    }
    if let Ok(p) = opus::packet::get_samples_per_frame(&opus_data, 48000) {
        println!("opus::packet::get_samples_per_frame: {:?}", p);
    }
    if let Ok(p) = opus::packet::get_bandwidth(&opus_data) {
        println!("opus::packet::get_bandwidth: {:?}", p);
    }
    if let Ok(p) = opus::packet::get_nb_channels(&opus_data) {
        println!("opus::packet::get_nb_channels: {:?}", p);
    }
    println!("======================================================");
}
