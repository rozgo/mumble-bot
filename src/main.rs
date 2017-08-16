#![allow(dead_code)]
#![allow(unused_imports)]
#![feature(rustc_private)]
#![feature(conservative_impl_trait)]

extern crate pretty_env_logger;
#[macro_use]
extern crate log;
extern crate clap;

extern crate openssl;
extern crate tokio_core;
extern crate tokio_openssl;
extern crate tokio_io;
extern crate tokio_file_unix;
extern crate tokio_timer;
extern crate protobuf;
extern crate byteorder;
extern crate opus;
extern crate chrono;
extern crate hyper;
extern crate hyper_tls;
extern crate rand;

extern crate warheadhateus;

extern crate serde;
extern crate toml;
#[macro_use]
extern crate serde_derive;

use clap::{Arg, App};
extern crate futures;

use std::fs;

use std::io::Cursor;
use std::io::{Write, Read, Error, ErrorKind};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use futures::{Sink, Stream};
use futures::future::{Future, ok, loop_fn, Loop};

use openssl::ssl::{SslContext, SslMethod, SSL_VERIFY_PEER};
use openssl::x509::X509_FILETYPE_PEM;

use tokio_io::AsyncRead;
use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;

use protobuf::Message;
use protobuf::{CodedOutputStream, CodedInputStream};

use byteorder::{BigEndian, LittleEndian, ReadBytesExt, WriteBytesExt};

mod mumble;
mod connector;
use connector::MumbleConnector;

mod varint;
use varint::VarintReader;
use varint::VarintWriter;

mod lex;
mod rnd;
mod config;
mod util;

fn app() -> App<'static, 'static> {
    App::new("mmo-mumble")
        .version("0.1.0")
        .about("Voice client bot!")
        .author("Alex Rozgo")
        .arg(Arg::with_name("addr")
            .short("a")
            .long("address")
            .help("Host to connect to address:port")
            .takes_value(true))
        .arg(Arg::with_name("cfg")
            .short("c")
            .long("config")
            .help("Path to config toml")
            .takes_value(true))
}

type BytesSender = futures::sync::mpsc::UnboundedSender<Vec<u8>>;
// type BytesReceiver = futures::sync::mpsc::UnboundedReceiver<Vec<u8>>;
type TCPReceiver = tokio_io::io::ReadHalf<connector::SslStream<tokio_core::net::TcpStream>>;

fn mumble_ping(mum_tx: BytesSender) -> impl Future<Item=(), Error=Error> {
    let timer = tokio_timer::Timer::default();
    timer.interval(Duration::from_secs(5)).fold(mum_tx, move |tx, _| {
        let ping = mumble::Ping::new();
        let s = ping.compute_size();
        let mut buf = vec![0u8; (s + 6) as usize];
        (&mut buf[0..]).write_u16::<BigEndian>(3).unwrap(); // Packet type: Ping
        (&mut buf[2..]).write_u32::<BigEndian>(s).unwrap();
        {
        let os = &mut CodedOutputStream::bytes(&mut buf[6..]);
        assert!(os.flush().is_ok());
        assert!(ping.write_to_with_cached_sizes(os).is_ok());
        }
        tx.send(buf)
        .map_err(|_| tokio_timer::TimerError::NoCapacity)
    })
    .map(|_| ())
    .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
}

fn mumble_decode(mut decoder: Box<opus::Decoder>, session: u32, aud_session: u64, aud_sequence: u64, opus_frame: Vec<u8>)
-> (Box<opus::Decoder>, Vec<u8>, bool) {

    let mut rdr = Cursor::new(opus_frame);

    let mut opus_frame = Vec::<u8>::new();
    let mut segments = 0;

    let mut opus_done = false;
    while let Ok(opus_header) = rdr.read_varint() {
        opus_done = if opus_header & 0x2000 == 0x2000 {true} else {false};
        opus_done = aud_sequence == 132;
        let opus_length = opus_header & 0x1FFF;
        println!("opus length: {} done: {}", opus_length, opus_done);
        let mut segment = vec![0u8; opus_length as usize];
        rdr.read_exact(&mut segment[..]).unwrap();
        opus_frame.write_all(&segment).unwrap();
        println!("opus size: {}", opus_length);
        segments = segments + 1;
    }

    util::opus_analyze(&opus_frame);

    let mut sample_pcm = vec![0i16; 480 * segments];
    
    let size = decoder.decode(&opus_frame[..], &mut sample_pcm[..], false).unwrap();
    println!("pcm size: {}", size);
    
    let mut pcm_data = Vec::<u8>::new();
    for s in 0..size {
        pcm_data.write_i16::<LittleEndian>(sample_pcm[s]).unwrap();
    }

    (decoder, pcm_data, opus_done)
}

// fn lex_request(
//     rx: TCPReceiver,
//     mum_tx: BytesSender,
//     config : &config::Config,
//     handle : &tokio_core::reactor::Handle)
// -> impl Future<Item=(), Error=Error> {
//         let (lex_tx, lex_rx) = futures::sync::mpsc::unbounded::<(Vec<u8>, u32)>();
//         let lex_task = lex_rx.fold(mum_tx.clone(), |mum_tx, (msg, session)| {

//             println!("%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%");
//             println!("LEX");
//             println!("%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%");

//             let mut req = lex::request(&config, session);
//             req.set_body(msg);
//             println!("req session: {}", session);

//             let mum_tx_new = mum_tx.clone();
//             let lex_client = lex::client(&handle);
//             lex_client.request(req).and_then(move |res| {

//                 // let mum_lex_tx = mum_lex_tx.clone();

//                 println!("req POST: {:?}", res.headers());
//                 let vox_data = Vec::<u8>::new();
//                 // TODO: remove cursor
//                 let vox_data = Cursor::<Vec<u8>>::new(vox_data);
//                 res.body().fold(vox_data, |mut vox_data, chunk| {
//                     println!("chuck: {}", chunk.len());
//                     vox_data.write_all(&chunk).unwrap();                    
//                     ok::<Cursor<Vec<u8>>, hyper::Error>(vox_data)
//                 })
//                 .and_then(move |vox_data| {

//                     let mum_tx = mum_tx.clone();

//                     // raw audio
//                     let vox_data = vox_data.into_inner();
//                     {
//                         let date = chrono::Local::now().format("%Y-%m-%d-%H-%M-%S");
//                         let file_name = format!("outgoing-{}.opus", date);
//                         let mut file = fs::File::create(file_name).unwrap();
//                         file.write_all(&vox_data).unwrap();
//                     }

//                     // pcm audio
//                     let mut vox_pcm = Vec::<i16>::new();
//                     {
//                         let mut cur = Cursor::new(&vox_data);
//                         while let Ok(i) = cur.read_i16::<LittleEndian>() {
//                             vox_pcm.push(i);
//                         }
//                     }

//                     println!("BODY DONE, size: {}", vox_data.len());

//                     // mumble_send_pcm(&vox_pcm, mum_tx)
//                     // .map_err(|_| hyper::Error::Header)

//                     type OpusFrames = Vec<(bool,Vec<u8>)>;

//                     // opus frames
//                     let mut encoder = opus::Encoder::new(16000, opus::Channels::Mono, opus::Application::Audio).unwrap();
//                     let mut opus = OpusFrames::new();
//                     let chunks = vox_pcm.chunks(320);
//                     for chunk in chunks {
//                         let mut frame = vec![0i16; 320];
//                         for (i, v) in chunk.iter().enumerate() {
//                             frame[i] = *v;
//                         }
//                         let frame = encoder.encode_vec(&frame, 4000).unwrap();
//                         // util::opus_analyze(&frame);
//                         opus.push((false, frame));
//                     }

//                     println!("opus count: {}", opus.len());

//                     if let Some((_, frame)) = opus.pop() {
//                         opus.push((true, frame));
//                     }
                    
//                     let p = (chrono::UTC::now().timestamp() as u64) & 0x000000000000FFFFu64;
//                     opus.reverse();
//                     let f = futures::stream::unfold((opus, p, mum_tx.clone()), |(state, seq, tx)| {
//                         let mut state = state;
//                     // futures::stream::iter(opus).for_each(move |opus| {
//                         match state.pop() {
//                             Some(opus) => {
//                                 let mut msg = mumble::UDPTunnel::new();
//                                 let (done, opus) = opus;
//                                 let aud_header = 0b100 << 5;
//                                 let mut data = Vec::<u8>::new();
//                                 data.write_u8(aud_header).unwrap();
//                                 data.write_varint(seq).unwrap();
//                                 let opus_len =
//                                     if done { 
//                                         println!("OPUS END: prev:{} next:{}", opus.len(), opus.len() as u64 | 0x2000);
//                                         opus.len() as u64 | 0x2000 }
//                                     else { opus.len() as u64 };
//                                 data.write_varint(opus_len).unwrap();
//                                 data.write_all(&opus).unwrap();
//                                 println!("p: {} opus: {} data: {}", seq, opus.len(), data.len());
//                                 // let mum_tx = mum_tx.clone();
//                                 msg.set_packet(data);
//                                 let s = msg.compute_size();
//                                 let mut buf = vec![0u8; (s + 6) as usize];
//                                 (&mut buf[0..]).write_u16::<BigEndian>(1).unwrap(); // Packet type: UDPTunnel
//                                 (&mut buf[2..]).write_u32::<BigEndian>(s).unwrap();
//                                 {
//                                 let os = &mut CodedOutputStream::bytes(&mut buf[6..]);
//                                 assert!(os.flush().is_ok());
//                                 assert!(msg.write_to_with_cached_sizes(os).is_ok());
//                                 }

//                                 // let fut = future::ok::<_, _>((yielded, state));

//                                 let tx_new = tx.clone();
//                                 let f = tx.send(buf)
//                                     // .map_err(|e| ())
//                                     .map_err(|_| Error::new(ErrorKind::Other, "mumble output"))
//                                     .and_then(move |_| ok::<_, Error>((0, (state, seq + 1, tx_new))))
//                                     // .map_err(|_| ());
//                                     ;
//                                 // let f = ok::<_, Error>((0, state));
//                                 // .map_err(|_| ())
//                                 // ;
//                                 Some(f)

//                             },
//                             _ => None,
//                             }
//                         // .map(|_| ())
//                         // .map_err(|_| opus::Error::new("who knows", opus::ErrorCode::BadArg))

                        
                        
                        
//                     });

//                     // let () = f;

//                     let p =
//                         f.into_future()
//                         .map_err(|_| hyper::Error::Header)
//                         .and_then(|_| ok::<(), hyper::Error>(()) );


//                     // let q = ok::<(), hyper::Error>(())
//                     // .and_then(|_| ok::<(), hyper::Error>(()) );


//                     // let () = p;
//                     // let () = q;

//                     p

//                     // let mut msg = mumble::TextMessage::new();
//                     // msg.set_actor(session);
//                     // msg.set_channel_id(vec![0]);
//                     // msg.set_message("just wanna say supercalifragilisticexpialidocious".to_string());
//                     // let s = msg.compute_size();
//                     // let mut buf = vec![0u8; (s + 6) as usize];
//                     // (&mut buf[0..]).write_u16::<BigEndian>(11).unwrap(); // Packet type: TextMessage
//                     // (&mut buf[2..]).write_u32::<BigEndian>(s).unwrap();
//                     // {
//                     // let os = &mut CodedOutputStream::bytes(&mut buf[6..]);
//                     // assert!(os.flush().is_ok());
//                     // assert!(msg.write_to_with_cached_sizes(os).is_ok());
//                     // }
//                     // mum_tx.send(buf)
//                     // .map_err(|_| hyper::Error::Header)
//                 })
//             })
//             .map(move |_| mum_tx_new)
//             .map_err(|_| ())
//         })
//         .map_err(|_| Error::new(ErrorKind::Other, "lex request"));

//     lex_request
// }

// fn lex_request(
//     rx: TCPReceiver,
//     tx: BytesSender,
//     config : &config::Config,
//     handle : &tokio_core::reactor::Handle)
// -> impl Future<Item=(), Error=Error> {

//     let lex_client = lex::client(&handle);

//     // lex request
//     let (lex_tx, lex_rx) = futures::sync::mpsc::unbounded::<(Vec<u8>, u32)>();

//     let lex_task = lex_rx.fold(tx, |tx, (msg, session)| {

//         let mut req = lex::request(&config, session);
//         req.set_body(msg);
//         println!("req session: {}", session);
 
//         lex_client.request(req).and_then(move |res| {

//             // let mum_lex_tx = mum_lex_tx.clone();

//             println!("req POST: {:?}", res.headers());
//             let vox_data = Vec::<u8>::new();
//             // TODO: remove cursor
//             let vox_data = Cursor::<Vec<u8>>::new(vox_data);
//             res.body().fold(vox_data, |mut vox_data, chunk| {
//                 println!("chuck: {}", chunk.len());
//                 vox_data.write_all(&chunk).unwrap();                    
//                 ok::<Cursor<Vec<u8>>, hyper::Error>(vox_data)
//             })
//             .and_then(move |vox_data| {
//                 ok(())
//             })

//         })
//         .map(|_| tx)
//         .map_err(|_| ())
//     })
//     // .map(|_| ok(()))
//     // .map_err(|_| Error::new(ErrorKind::Other, "lex request"))
//     // // .boxed()
//     ;

//     // let () = lex_task;

//     // 
//     lex_task
//     .and_then(|_| {
//         ok(())
//     })
//     .map_err(|_| Error::new(ErrorKind::Other, "lex request"))

    
// }

fn voice_buffer() -> (futures::sync::mpsc::UnboundedSender<(Vec<u8>, u32, bool)>, Box<Future<Item=(), Error=Error>>) {
    let (vox_tx, vox_rx) = futures::sync::mpsc::unbounded::<(Vec<u8>, u32, bool)>();
    let vox_buf : Option<_> = None;
    let voice_buffer = vox_rx.fold(vox_buf, move |writer, (msg, session, done)| {
        let mut writer = 
            match writer {
                Some (writer) => writer,
                None => Vec::<u8>::new(),
            };
        writer.write_all(&msg).unwrap();
        if done {
            let vox_data = writer;
            {
                let date = chrono::Local::now().format("%Y-%m-%d-%H-%M-%S");
                let file_name = format!("incoming-{}.opus", date);
                let mut file = fs::File::create(file_name).unwrap();
                file.write_all(&vox_data).unwrap();
            }
            // let lex_tx = lex_tx.clone();
            // lex_tx.send((vox_data, session))
            // // .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
            // .map_err(|_| ())
            // .and_then(|_| ok::<Option<Vec<u8>>, ()>(None))
            // .boxed()
            ok::<Option<Vec<u8>>, ()>(None)
            .boxed()
        }
        else {
            ok::<Option<Vec<u8>>, ()>(Some(writer))
            .boxed()
        }
        // // .map(|e| ok(e))
        // .map_err(|_| ())
    })
    .map(|_| ())
    .map_err(|_| Error::new(ErrorKind::Other, "dumping to file"));
    (vox_tx, voice_buffer.boxed())
}

fn mumble_loop(
    rx: TCPReceiver,
    tx: futures::sync::mpsc::UnboundedSender<(Vec<u8>, u32, bool)>) 
    -> Box<Future<Item=(), Error=Error>> {

    let decoder = Box::new(opus::Decoder::new(48000, opus::Channels::Mono).unwrap());

    loop_fn((rx, tx, 0, 0, decoder), |(rx, tx, counter, session, decoder)| {

        // packet type
        tokio_io::io::read_exact(rx, [0u8; 2])
        .and_then(|(rx, buf)| {
            let mut rdr = Cursor::new(&buf);
            let mum_type = rdr.read_u16::<BigEndian>().unwrap();
            trace!("mum_type: {}", mum_type);
            tokio_io::io::read_exact(rx, [0u8; 4])
            .and_then(move |(rx, buf)| {
                ok((rx, buf, mum_type))
            })
        })

        // packet length
        .and_then(|(rx, buf, mum_type)| {
            let mut rdr = Cursor::new(&buf);
            let mum_length = rdr.read_u32::<BigEndian>().unwrap();
            tokio_io::io::read_exact(rx, vec![0u8; mum_length as usize])
            .and_then(move |(rx, buf)| {
                ok((rx, buf, mum_type))
            })
        })

        // packet payload
        .and_then(move |(rx, buf, mum_type)| {

            let mut inp = CodedInputStream::from_bytes(&buf);

            match mum_type {
                0 => { // Version
                    let mut msg = mumble::Version::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("version: {:?}", msg);
                    ok((rx, tx, session, counter, decoder))
                    .boxed()
                },
                1 => { // UDPTunnel
                    let mut rdr = Cursor::new(&buf);
                    let aud_header = rdr.read_u8().unwrap();
                    // println!("incoming aud_header: {}", aud_header);
                    let aud_type = aud_header & 0b11100000;
                    let aud_target = aud_header & 0b00011111;
                    debug!("type: {} target: {}", aud_type, aud_target);                
                    match aud_type {
                        
                        128 => { // OPUS encoded voice data                                
                            let aud_session = rdr.read_varint().unwrap();
                            let aud_sequence = rdr.read_varint().unwrap();
                            println!("session: {} sequence: {}", aud_session, aud_sequence);
                            let mut opus_frame = Vec::<u8>::new();
                            rdr.read_to_end(&mut opus_frame).unwrap();
                            let (decoder, pcm, done) = mumble_decode(decoder, session, aud_session, aud_sequence, opus_frame);
                            tx.send((pcm, session, done))
                            .and_then(move |tx| {
                                ok((rx, tx, session, counter, decoder))
                            })
                            .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
                            .boxed()
                        },
                        32 => { // Ping
                            ok((rx, tx, session, counter, decoder))
                            .boxed()
                        },
                        _ => {
                            panic!("aud_type");
                        }
                    }
                },
                5 => { // ServerSync
                    let mut msg = mumble::ServerSync::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("ServerSync: {:?}", msg);
                    ok((rx, tx, msg.get_session(), counter, decoder))
                    .boxed()
                },
                7 => { // ChannelState
                    let mut msg = mumble::ChannelState::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("ChannelState: {:?}", msg);
                    ok((rx, tx, session, counter, decoder))
                    .boxed()
                },
                9 => { // UserState
                    let mut msg = mumble::UserState::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("UserState: {:?}", msg);
                    ok((rx, tx, session, counter, decoder))
                    .boxed()
                },
                11 => { // TextMessage
                    let mut msg = mumble::TextMessage::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("TextMessage: {:?}", msg);
                    ok((rx, tx, session, counter, decoder))
                    .boxed()
                },
                15 => { // CryptSetup
                    let mut msg = mumble::CryptSetup::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("CryptSetup: {:?}", msg);
                    ok((rx, tx, session, counter, decoder))
                    .boxed()
                },
                20 => { // PermissionQuery
                    let mut msg = mumble::PermissionQuery::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("PermissionQuery: {:?}", msg);
                    ok((rx, tx, session, counter, decoder))
                    .boxed()
                },
                21 => { // CodecVersion
                    let mut msg = mumble::CodecVersion::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("CodecVersion: {:?}", msg);
                    ok((rx, tx, session, counter, decoder))
                    .boxed()
                },
                24 => { // ServerConfig
                    let mut msg = mumble::ServerConfig::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("ServerConfig: {:?}", msg);
                    ok((rx, tx, session, counter, decoder))
                    .boxed()
                },
                _ => {
                    panic!("wtf")
                }
            }
        })
        .and_then(move |(rx, tx, session, counter, decoder)| {
            Ok(Loop::Continue((rx, tx, 0, 0, decoder)))
        })
    })
    .boxed()
}

fn main() {
    pretty_env_logger::init().unwrap();

    let matches = app().get_matches();

    let config: config::Config = {
        let config_file = matches.value_of("cfg").unwrap_or("Config.toml");
        let mut config_file = fs::File::open(config_file).unwrap();
        let mut config = String::new();
        config_file.read_to_string(&mut config).unwrap();
        toml::from_str(&config).unwrap()
    };

    let mumble_server = &config.mumble.server;
    let access_key_id = &config.aws.access_key_id;
    let secret_access_key = &config.aws.secret_access_key;
    println!("mumble_server {}", mumble_server);
    println!("access_key_id {}", access_key_id);
    println!("secret_access_key {}", secret_access_key);

    let addr_str = matches.value_of("addr").unwrap_or("127.0.0.1:8080");
    let addr = addr_str.parse::<SocketAddr>().unwrap();

    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let client = TcpStream::connect(&addr, &handle);

    let app_logic = client.and_then(|socket| {
        let path = Path::new("mumble.pem");
        let mut ctx = SslContext::builder(SslMethod::tls()).unwrap();
        ctx.set_verify_callback(SSL_VERIFY_PEER, |_, _| true);
        assert!(ctx.set_certificate_file(&path, X509_FILETYPE_PEM).is_ok());
        let ctx = ctx.build();
        let connector = MumbleConnector(ctx);
        connector.connect_async(addr_str, socket)
        .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))

    }).and_then(|stream| { // Version
        let mut version = mumble::Version::new();
        version.set_version(66052);
        version.set_release("1.2.4-0.2ubuntu1.1".to_string());
        version.set_os("X11".to_string());
        version.set_os_version("Ubuntu 14.04.5 LTS".to_string());
        let s = version.compute_size();
        let mut buf = vec![0u8; (s + 6) as usize];
        (&mut buf[0..]).write_u16::<BigEndian>(0).unwrap(); // Packet type: Version
        (&mut buf[2..]).write_u32::<BigEndian>(s).unwrap();
        {
        let os = &mut CodedOutputStream::bytes(&mut buf[6..]);
        assert!(os.flush().is_ok());
        assert!(version.write_to_with_cached_sizes(os).is_ok());
        }
        tokio_io::io::write_all(stream, buf)

    }).and_then(|(stream, _)| { // Authenticate
        let mut auth = mumble::Authenticate::new();
        auth.set_username("lyric".to_string());
        auth.set_opus(true);
        let s = auth.compute_size();
        let mut buf = vec![0u8; (s + 6) as usize];
        (&mut buf[0..]).write_u16::<BigEndian>(2).unwrap(); // Packet type: Authenticate
        (&mut buf[2..]).write_u32::<BigEndian>(s).unwrap();
        {
        let os = &mut CodedOutputStream::bytes(&mut buf[6..]);
        assert!(os.flush().is_ok());
        assert!(auth.write_to_with_cached_sizes(os).is_ok());
        }
        tokio_io::io::write_all(stream, buf)

    }).and_then(|(stream, _)| {

        let (mum_reader, mum_writer) = stream.split();

        // mumble writer
        let (mum_tx, mum_rx) = futures::sync::mpsc::unbounded::<Vec<u8>>();
        let mum_writer = mum_rx.fold(mum_writer, move |writer, msg : Vec<u8>| {
            println!("MSG {:?}", msg.len());
            tokio_io::io::write_all(writer, msg)
            .map(|(writer, _)| writer)
            .map_err(|_| ())
        })
        .map_err(|_| Error::new(ErrorKind::Other, "writing to tcp"));

        // lex request
        let (lex_tx, lex_rx) = futures::sync::mpsc::unbounded::<(Vec<u8>, u32)>();
        let lex_task = lex_rx.fold(mum_tx.clone(), |mum_tx, (msg, session)| {

            println!("%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%");
            println!("LEX");
            println!("%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%");

            let mut req = lex::request(&config, session);
            req.set_body(msg);
            println!("req session: {}", session);

            let mum_tx_new = mum_tx.clone();
            let lex_client = lex::client(&handle);
            lex_client.request(req).and_then(move |res| {

                // let mum_lex_tx = mum_lex_tx.clone();

                println!("req POST: {:?}", res.headers());
                let vox_data = Vec::<u8>::new();
                // TODO: remove cursor
                let vox_data = Cursor::<Vec<u8>>::new(vox_data);
                res.body().fold(vox_data, |mut vox_data, chunk| {
                    println!("chuck: {}", chunk.len());
                    vox_data.write_all(&chunk).unwrap();                    
                    ok::<Cursor<Vec<u8>>, hyper::Error>(vox_data)
                })
                .and_then(move |vox_data| {

                    let mum_tx = mum_tx.clone();

                    // raw audio
                    let vox_data = vox_data.into_inner();
                    {
                        let date = chrono::Local::now().format("%Y-%m-%d-%H-%M-%S");
                        let file_name = format!("outgoing-{}.opus", date);
                        let mut file = fs::File::create(file_name).unwrap();
                        file.write_all(&vox_data).unwrap();
                    }

                    // pcm audio
                    let mut vox_pcm = Vec::<i16>::new();
                    {
                        let mut cur = Cursor::new(&vox_data);
                        while let Ok(i) = cur.read_i16::<LittleEndian>() {
                            vox_pcm.push(i);
                        }
                    }

                    println!("BODY DONE, size: {}", vox_data.len());

                    // mumble_send_pcm(&vox_pcm, mum_tx)
                    // .map_err(|_| hyper::Error::Header)

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
                    
                    let p = (chrono::UTC::now().timestamp() as u64) & 0x000000000000FFFFu64;
                    opus.reverse();
                    let f = futures::stream::unfold((opus, p, mum_tx.clone()), |(state, seq, tx)| {
                        let mut state = state;
                    // futures::stream::iter(opus).for_each(move |opus| {
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
                                    .map_err(|_| Error::new(ErrorKind::Other, "mumble output"))
                                    .and_then(move |_| ok::<_, Error>((0, (state, seq + 1, tx_new))))
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

                        
                        
                        
                    });

                    // let () = f;

                    let p =
                        f.into_future()
                        .map_err(|_| hyper::Error::Header)
                        .and_then(|_| ok::<(), hyper::Error>(()) );


                    // let q = ok::<(), hyper::Error>(())
                    // .and_then(|_| ok::<(), hyper::Error>(()) );


                    // let () = p;
                    // let () = q;

                    p

                    // let mut msg = mumble::TextMessage::new();
                    // msg.set_actor(session);
                    // msg.set_channel_id(vec![0]);
                    // msg.set_message("just wanna say supercalifragilisticexpialidocious".to_string());
                    // let s = msg.compute_size();
                    // let mut buf = vec![0u8; (s + 6) as usize];
                    // (&mut buf[0..]).write_u16::<BigEndian>(11).unwrap(); // Packet type: TextMessage
                    // (&mut buf[2..]).write_u32::<BigEndian>(s).unwrap();
                    // {
                    // let os = &mut CodedOutputStream::bytes(&mut buf[6..]);
                    // assert!(os.flush().is_ok());
                    // assert!(msg.write_to_with_cached_sizes(os).is_ok());
                    // }
                    // mum_tx.send(buf)
                    // .map_err(|_| hyper::Error::Header)
                })
            })
            .map(move |_| mum_tx_new)
            .map_err(|_| ())
        })
        .map_err(|_| Error::new(ErrorKind::Other, "lex request"));

        let (vox_tx, vox_task) = voice_buffer();        

        let mumble_ping = mumble_ping(mum_tx.clone());
        let mumble_loop = mumble_loop(mum_reader, vox_tx);
        // let (vox_tx, vox_buffer) = voice_buffer();

        // Future::join4(mumble_ping, mumble_loop, mum_writer, vox_task)
        mumble_ping
    });

    core.run(app_logic).unwrap();
}
