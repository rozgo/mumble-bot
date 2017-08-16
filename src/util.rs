
use opus;
use std::io::{Cursor, Write, Error, ErrorKind};
use byteorder::{BigEndian, LittleEndian, ReadBytesExt, WriteBytesExt};
// use byteorder::{LittleEndian, ReadBytesExt};

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

// fn send_version<S, B, R>(stream : S, buf : B) -> R {
//     let mut version = mumble::Version::new();
//     version.set_version(66052);
//     version.set_release("1.2.4-0.2ubuntu1.1".to_string());
//     version.set_os("X11".to_string());
//     version.set_os_version("Ubuntu 14.04.5 LTS".to_string());
//     let s = version.compute_size();
//     let mut buf = vec![0u8; (s + 6) as usize];
//     (&mut buf[0..]).write_u16::<BigEndian>(0).unwrap(); // Packet type: Version
//     (&mut buf[2..]).write_u32::<BigEndian>(s).unwrap();
//     {
//         let os = &mut CodedOutputStream::bytes(&mut buf[6..]);
//         assert!(os.flush().is_ok());
//         assert!(version.write_to_with_cached_sizes(os).is_ok());
//     }
//     tokio_io::io::write_all(stream, buf)
// }


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

pub fn raw_to_pcm(raw : &Vec<u8>) -> Vec<i16> {
    let mut pcm = Vec::<i16>::new();
    {
        let mut cur = Cursor::new(raw);
        while let Ok(i) = cur.read_i16::<LittleEndian>() {
            pcm.push(i);
        }
    }
    pcm
}

// pub struct PCM_Sender {
//     pcm : Vec<i16>,
//     tx : futures::sync::mpsc::UnboundedSender<Vec<u8>>,
// }

// impl PCM_Sender {

//     pub fn new(pcm : Vec<i16>, tx : futures::sync::mpsc::UnboundedSender<Vec<u8>>) -> PCM_Sender {
//         PCM_Sender{pcm : pcm, tx : tx}
//     }
// }

// impl Future for PCM_Sender {
//     type Item = ();
//     type Error = futures::sync::mpsc::SendError<Vec<u8>>;

//     fn poll(&mut self) -> futures::Poll<(), futures::sync::mpsc::SendError<Vec<u8>>> {
//         self.tx.poll_complete()
//         // Ok(Async::NotReady)
//     }
// }

// pub fn pcm_to_raw(pcm : &Vec<i16>) -> Vec<u8> {

// }

// pub fn mumble_send_pcm(vox_pcm : &Vec<i16>, tx : futures::sync::mpsc::UnboundedSender<Vec<u8>>) -> impl Future<Item = (), Error = futures::sync::mpsc::SendError<Vec<u8>>> {
pub fn mumble_send_pcm(vox_pcm : &Vec<i16>, tx : futures::sync::mpsc::UnboundedSender<Vec<u8>>) -> impl Future<Item = (), Error = Error> {
// pub fn mumble_send_pcm<T, F, Fut>(vox_pcm : &Vec<i16>, tx : futures::sync::mpsc::UnboundedSender<Vec<u8>>) -> futures::stream::Unfold<T, F, Fut>
// where
//     Fut : futures::future::IntoFuture,
//  {


    type OpusFrames = Vec<(bool,Vec<u8>)>;

    // opus frames
    let mut encoder = opus::Encoder::new(16000, opus::Channels::Mono, opus::Application::Audio).unwrap();
    let mut opus = OpusFrames::new();
    let chunks = vox_pcm.chunks(320);
    for chunk in chunks {
        let mut frame = vec![0i16; 320];
        for (i, v) in chunk.iter().enumerate() {
            frame[i] = *v;
        }
        let frame = encoder.encode_vec(&frame, 4000).unwrap();
        // util::opus_analyze(&frame);
        opus.push((false, frame));
    }

    println!("opus count: {}", opus.len());

    if let Some((_, frame)) = opus.pop() {
        opus.push((true, frame));
    }

    type UnfoldItem = (Vec<(bool, Vec<u8>)>, u64, futures::sync::mpsc::UnboundedSender<Vec<u8>>);

    let p = (chrono::UTC::now().timestamp() as u64) & 0x000000000000FFFFu64;
    opus.reverse();
    let f = futures::stream::unfold((opus, p, tx.clone()), |(state, seq, tx)| {
        let mut state = state;
        match state.pop() {
            Some(opus) => {
                let mut msg = mumble::UDPTunnel::new();
                let (done, opus) = opus;
                let aud_header = 0b100 << 5;
                let mut data = Vec::<u8>::new();
                data.write_u8(aud_header).unwrap();
                data.write_varint(seq).unwrap();
                let opus_len =
                    if done { 
                        println!("OPUS END: prev:{} next:{}", opus.len(), opus.len() as u64 | 0x2000);
                        opus.len() as u64 | 0x2000 }
                    else { opus.len() as u64 };
                data.write_varint(opus_len).unwrap();
                data.write_all(&opus).unwrap();
                println!("p: {} opus: {} data: {}", seq, opus.len(), data.len());
                // let mum_tx = mum_tx.clone();
                msg.set_packet(data);
                let s = msg.compute_size();
                let mut buf = vec![0u8; (s + 6) as usize];
                (&mut buf[0..]).write_u16::<BigEndian>(1).unwrap(); // Packet type: UDPTunnel
                (&mut buf[2..]).write_u32::<BigEndian>(s).unwrap();
                {
                let os = &mut CodedOutputStream::bytes(&mut buf[6..]);
                assert!(os.flush().is_ok());
                assert!(msg.write_to_with_cached_sizes(os).is_ok());
                }

                // let fut = future::ok::<_, _>((yielded, state));

                let tx_new = tx.clone();
                let f = tx.send(buf)
                    // .map_err(|e| ())
                    // .map_err(|_| Error::new(ErrorKind::Other, "dumping to file"))
                    .and_then(move |_| ok::<_, _>((0, (state, seq + 1, tx_new))))
                    // .map_err(|_| ());
                    ;
                // let f = ok::<_, Error>((0, state));
                // .map_err(|_| ())
                // ;
                Some(f)

            },
            _ => None,
            }
        // .map(|_| ())
        // .map_err(|_| opus::Error::new("who knows", opus::ErrorCode::BadArg))

        
        
        
    })
    // .map_err(|_| Error::new(ErrorKind::Other, "sending pcm"))
    ;

    f.fold((), |acc, i| {
        println!(">>>> {:?}", i);
        ok(acc)
        // .map_err(|e| e.into())
        // .map_err(|_| err_str)
    })
    .map_err(|_| Error::new(ErrorKind::Other, "sending pcm"))
    
    // f
    // Box::<Future<Item=(), Error=Error>>::new(p)
    

    // .map_err(|_| Error::new(ErrorKind::Other, "dumping to file"))
    // .map(|i: u32| ())
        // .into_future()

    // ok::<_, Error>(f)
        // .map_err(|_| Error::new(ErrorKind::Other, "dumping to file"))
        // .and_then(|_| ok::<(), Error>(()) )
}

