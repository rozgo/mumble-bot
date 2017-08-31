#![allow(dead_code)]
#![allow(unused_imports)]
#![feature(conservative_impl_trait)]

extern crate pretty_env_logger;
#[macro_use]
extern crate log;
extern crate clap;

extern crate openssl;
extern crate tokio_core;
//extern crate tokio_openssl;
extern crate tokio_io;
//extern crate tokio_file_unix;
extern crate tokio_timer;
extern crate protobuf;
extern crate byteorder;
extern crate opus;
extern crate chrono;
//extern crate hyper;
//extern crate hyper_tls;
extern crate rand;

//extern crate warheadhateus;

//extern crate serde;
extern crate toml;
#[macro_use]
extern crate serde_derive;

use clap::{Arg, App};
extern crate futures;

use rand::distributions::{Range};

use std::fs;
use std::io::Cursor;
use std::io::{Write, Read, Error, ErrorKind};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use std::collections::HashMap;

use futures::{Sink, Stream};
use futures::future::{Future, ok, loop_fn, IntoFuture, Loop};

use openssl::ssl::{SslContext, SslMethod, SSL_VERIFY_PEER};
use openssl::x509::X509_FILETYPE_PEM;

use tokio_io::AsyncRead;
use tokio_core::net::TcpStream;
use tokio_core::reactor::{Core, Timeout};

use protobuf::Message;
use protobuf::{CodedOutputStream, CodedInputStream};

use byteorder::{BigEndian, LittleEndian, ReadBytesExt, WriteBytesExt};

mod mumble;
mod connector;
use connector::MumbleConnector;

mod varint;
use varint::VarintReader;
use varint::VarintWriter;

//mod lex;
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

type BytesSender = futures::sync::mpsc::Sender<Vec<u8>>;
// type BytesReceiver = futures::sync::mpsc::UnboundedReceiver<Vec<u8>>;
type TCPReceiver = tokio_io::io::ReadHalf<connector::SslStream<tokio_core::net::TcpStream>>;

fn mumble_ping(mum_tx: BytesSender) -> impl Future<Item = (), Error = Error> {
    tokio_timer::Timer::default()
        .interval(Duration::from_secs(5))
        .fold(mum_tx, move |tx, _| {
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

struct RemoteSession {
    decoder: Box<opus::Decoder>,
    session: u64,
    sequence: u64,
}

type RemoteSessions = HashMap<u64, Box<RemoteSession>>;

struct LocalSession {
    session: u64,
}

fn mumble_decode(remote_session: &mut Box<RemoteSession>, opus_frame: Vec<u8>) -> (Vec<u8>, bool) {

    //println!("!!__@__!!__@__!!__@__!!__@__!!__@__!!__@__!!__@__!!__@__!!__@__!!__@__");

    let mut rdr = Cursor::new(opus_frame);

    let mut opus_frame = Vec::<u8>::new();
    let mut segments = 0;

    let mut opus_done = false;
    while let Ok(opus_header) = rdr.read_varint() {
        opus_done = if opus_header & 0x2000 == 0x2000 {
            true
        } else {
            false
        };
//        opus_done = aud_sequence == 132;
        let opus_length = opus_header & 0x1FFF;
        //println!("opus length: {} done: {}", opus_length, opus_done);
        let mut segment = vec![0u8; opus_length as usize];
        rdr.read_exact(&mut segment[..]).unwrap();
        opus_frame.write_all(&segment).unwrap();
        //println!("opus size: {}", opus_length);
        segments = segments + 1;
    }

    util::opus_analyze(&opus_frame);

    let mut sample_pcm = vec![0i16; 320 * segments];

    let size: usize = remote_session.decoder.decode(&opus_frame[..], &mut sample_pcm[..], false).unwrap();
    //println!("pcm size: {}", size);

    let mut pcm_data = Vec::<u8>::new();
    for s in 0..size {
        pcm_data.write_i16::<LittleEndian>(sample_pcm[s]).unwrap();
    }

    (pcm_data, opus_done)
}

fn manage_remote_sessions(session_id: u64, sessions: Box<RemoteSessions>) -> Box<RemoteSessions> {
    if sessions.contains_key(&session_id) {
        sessions
    }
    else {
        let session = Box::new(RemoteSession {
            decoder: Box::new(opus::Decoder::new(16000, opus::Channels::Mono).unwrap()),
            session: session_id,
            sequence: 0,
        });
        let mut sessions = sessions;
        sessions.insert(session_id, session);
        sessions
    }
    //TODO: garbage collect sessions
}

fn test_impl<'a>() -> impl Future<Item = (), Error = Error> + 'a {
    match false {
        true => ok(true),
        false => ok(false),
    }
        .map(|_| ())
}

fn do_that<'a>(vox_tx: futures::sync::mpsc::Sender<Vec<u8>>) -> impl Future<Item = (), Error = Error> + 'a {
    ok(())
}

fn mumble_loop<'a>(remote_sessions: Box<RemoteSessions>,
                   rx: TCPReceiver,
                   vox_tx: futures::sync::mpsc::Sender<Vec<u8>>)
               -> impl Future<Item = (), Error = Error> + 'a {

    loop_fn((rx, vox_tx, LocalSession{session: 0}, remote_sessions), |(rx, tx, local_session, remote_sessions)|

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
            .and_then(move |(rx, buf)| ok((rx, buf, mum_type)))
        })

        // packet payload
        .and_then(move |(rx, buf, mum_type)| {

            //println!("mum_type: {}", mum_type);

            let mut inp = CodedInputStream::from_bytes(&buf);

            match mum_type {
                0 => { // Version
                    let mut msg = mumble::Version::new();
                    msg.merge_from(&mut inp).unwrap();
                    //println!("version: {:?}", msg);
                    ok((rx, tx, local_session, remote_sessions)).boxed()
                },
                1 => { // UDPTunnel
                    let mut rdr = Cursor::new(&buf);
                    let aud_header = rdr.read_u8().unwrap();
                    // println!("incoming aud_header: {}", aud_header);
                    let aud_type = aud_header & 0b11100000;
                    let aud_target = aud_header & 0b00011111;
                    debug!("type: {} target: {}", aud_type, aud_target);
                    let r =
                    match aud_type {

                        128 => { // OPUS encoded voice data

                            let aud_session = rdr.read_varint().unwrap();
                            let aud_sequence = rdr.read_varint().unwrap();
                            //println!("session: {} sequence: {}", aud_session, aud_sequence);

                            let mut remote_sessions = manage_remote_sessions(aud_session, remote_sessions);

                            let mut opus_frame = Vec::<u8>::new();
                            rdr.read_to_end(&mut opus_frame).unwrap();
                            let (pcm, done) = {
                                let remote_session = remote_sessions.get_mut(&aud_session).unwrap();
                                mumble_decode(remote_session, opus_frame)
                            };

//                            ok((rx, tx, local_session, remote_sessions)).boxed()
//                                .and_then(|o| ok(o))
//                                .boxed()

                            tx.send(pcm)
                            .and_then(move |tx| {
                                ok((rx, tx, local_session, remote_sessions))
                            })
                                .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
                                .boxed()
//                                .map(|_| ok(()))

                        },
                        32 => { // Ping
                            ok((rx, tx, local_session, remote_sessions)).boxed()
                        },
                        _ => {
                            panic!("aud_type");
                        }
                    };
                    r
                },
                3 => { // Ping
                    let mut msg = mumble::Ping::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("Ping: {:?}", msg);
                    ok((rx, tx, local_session, remote_sessions)).boxed()
                },
                5 => { // ServerSync
                    let mut msg = mumble::ServerSync::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("ServerSync: {:?}", msg);
                    ok((rx, tx, LocalSession{session: msg.get_session() as u64}, remote_sessions)).boxed()
                },
                7 => { // ChannelState
                    let mut msg = mumble::ChannelState::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("ChannelState: {:?}", msg);
                    ok((rx, tx, local_session, remote_sessions)).boxed()
                },
                8 => { // UserRemove
                    let mut msg = mumble::UserRemove::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("UserRemove: {:?}", msg);
                    ok((rx, tx, local_session, remote_sessions)).boxed()
                },
                9 => { // UserState
                    let mut msg = mumble::UserState::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("UserState: {:?}", msg);
                    ok((rx, tx, local_session, remote_sessions)).boxed()
                },
                11 => { // TextMessage
                    let mut msg = mumble::TextMessage::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("TextMessage: {:?}", msg);
                    ok((rx, tx, local_session, remote_sessions)).boxed()
                },
                15 => { // CryptSetup
                    let mut msg = mumble::CryptSetup::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("CryptSetup: {:?}", msg);
                    ok((rx, tx, local_session, remote_sessions)).boxed()
                },
                20 => { // PermissionQuery
                    let mut msg = mumble::PermissionQuery::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("PermissionQuery: {:?}", msg);
                    ok((rx, tx, local_session, remote_sessions)).boxed()
                },
                21 => { // CodecVersion
                    let mut msg = mumble::CodecVersion::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("CodecVersion: {:?}", msg);
                    ok((rx, tx, local_session, remote_sessions)).boxed()
                },
                24 => { // ServerConfig
                    let mut msg = mumble::ServerConfig::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("ServerConfig: {:?}", msg);
                    ok((rx, tx, local_session, remote_sessions)).boxed()
                },
                _ => {
                    panic!("msg type oops")
                }
            }
        })
        .and_then(move |(rx, tx, local_session, remote_sessions)| {
            Ok(Loop::Continue((rx, tx, local_session, remote_sessions)))
        })
    )
}

pub fn say<'a>(rx: futures::sync::mpsc::Receiver<Vec<u8>>,
               tx: futures::sync::mpsc::Sender<Vec<u8>>)
    -> impl Future<Item = (), Error = Error> + 'a {

    // Hz * channel * ms / 1000
    let sample_channels : u32 = 1;
    let sample_rate : u32 = 16000;
    let sample_ms : u32 = 10;
    let sample_size: u32 = sample_rate * sample_channels * sample_ms / 1000;

    println!("sample channels: {} rate: {} ms: {} size: {}", sample_channels, sample_rate, sample_ms, sample_size);

    let mut encoder = opus::Encoder::new(sample_rate,
                                         opus::Channels::Mono,
                                         opus::Application::Voip).unwrap();

    let mut sequence = chrono::UTC::now().timestamp() as u64;

    let mut chunk_buffer = vec![0i16; sample_size as usize];

    rx.map(|segment| {
        futures::stream::iter_ok::<_,()>(segment)
    })
    .flatten()
    .chunks(2)
    .and_then(|raw| {
        let pcm = (&raw[..]).read_i16::<LittleEndian>().unwrap();
        ok::<i16, ()>(pcm)
    })
    .chunks(sample_size as usize)
    .fold(tx, move |tx, chunk| {
        let mut chunk = Vec::from(chunk);
        chunk.resize(sample_size as usize, 0);
        let frame = encoder.encode_vec(&chunk[..], 50000).unwrap();

        sequence = sequence + 1;

        let done = false;

        let aud_header = 0b100 << 5;
        let data = Vec::<u8>::new();
        let mut data = Cursor::new(data);
        data.write_u8(aud_header).unwrap();
        data.write_varint(sequence).unwrap();
        let opus_len =
            if done {
                frame.len() as u64 | 0x2000 }
                else { frame.len() as u64 };
        data.write_varint(opus_len).unwrap();
        data.write_all(&frame).unwrap();
        let data = data.into_inner();

        let s = data.len();
        let mut buf = vec![0u8; (s + 6) as usize];
        (&mut buf[0..]).write_u16::<BigEndian>(1).unwrap(); // Packet type: UDPTunnel
        (&mut buf[2..]).write_u32::<BigEndian>(s as u32).unwrap();
        (&mut buf[6..]).write(&data[..]).unwrap();

//        println!("saying");

        tx.send(buf)
            .map(|tx| tx)
            .map_err(|_| ())
    })
    .map(|_| ())
    .map_err(|_| Error::new(ErrorKind::Other, "vox out"))
}

pub fn say_something(tx: futures::sync::mpsc::Sender<Vec<u8>>) {

    std::thread::sleep(std::time::Duration::from_secs(5));

    let mut pcm_file = fs::File::open("C:\\temp\\thx.raw").unwrap();
    let mut pcm_data = Vec::<u8>::new();
    pcm_file.read_to_end(&mut pcm_data).unwrap();

    let mut pcm_chunks = Vec::<i16>::new();
    pcm_chunks.reserve_exact(pcm_data.len());
    {
        let mut cur = Cursor::new(&pcm_data);
        while let Ok(i) = cur.read_i16::<LittleEndian>() {
            pcm_chunks.push(i);
        }
    }

    // Hz * channel * ms / 1000
    let sample_channels : u32 = 1;
    let sample_rate : u32 = 48000;
    let sample_ms : u32 = 10;
    let sample_size: u32 = sample_rate * sample_channels * sample_ms / 1000;

    let mut encoder = opus::Encoder::new(sample_rate, opus::Channels::Mono, opus::Application::Audio).unwrap();
    let mut opus_frames = Vec::<Vec<u8>>::new();

    let chunks = pcm_chunks.chunks(sample_size as usize);
    for chunk in chunks {
        let frame = encoder.encode_vec(&chunk[..], 50000).unwrap();
        opus_frames.push(frame);
    }

    let mut p = chrono::UTC::now().timestamp() as u64;

    let mut tx = tx;

    for frame in opus_frames {

        p = p + 1;

        let done = false;

        let aud_header = 0b100 << 5;
        let data = Vec::<u8>::new();
        let mut data = Cursor::new(data);
        data.write_u8(aud_header).unwrap();
        data.write_varint(p).unwrap();
        let opus_len =
            if done {
                frame.len() as u64 | 0x2000 }
                else { frame.len() as u64 };
        data.write_varint(opus_len).unwrap();
        data.write_all(&frame).unwrap();
        let data = data.into_inner();

        let s = data.len();
        let mut buf = vec![0u8; (s + 6) as usize];
        (&mut buf[0..]).write_u16::<BigEndian>(1).unwrap(); // Packet type: UDPTunnel
        (&mut buf[2..]).write_u32::<BigEndian>(s as u32).unwrap();
        (&mut buf[6..]).write(&data[..]).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(sample_ms as u64));

        let tx0 = tx.send(buf);
        tx = tx0.wait().unwrap();
    }
}

pub fn cmd() {
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
    let mut core = Core::new().unwrap();
    let handle = core.handle();

//    let (vox_tx, _) = futures::sync::mpsc::channel::<Vec<u8>>(0);
    let (app_logic, mum_tx, vox_rx) = run(mumble_server, &handle);

    let tx = mum_tx.clone();
    std::thread::spawn(|| {
        say_something(tx);
    });

    core.run(app_logic).unwrap();
}

pub fn run<'a>(mumble_server: &'a String,
               handle: &tokio_core::reactor::Handle)
    -> (impl Future<Item = (), Error = Error> + 'a, futures::sync::mpsc::Sender<Vec<u8>>, futures::sync::mpsc::Receiver<Vec<u8>>) {

    let addr = mumble_server.parse::<SocketAddr>().unwrap();
    let (mum_tx, mum_rx) = futures::sync::mpsc::channel::<Vec<u8>>(0);
    let mum_tx0 = mum_tx.clone();
    let (vox_tx, vox_rx) = futures::sync::mpsc::channel::<Vec<u8>>(0);

    let comm = TcpStream::connect(&addr, &handle).and_then(move|socket| {
        println!("Connecting to mumble_server: {}", mumble_server);
//        let path = Path::new("mumble.pem");
        let mut ctx = SslContext::builder(SslMethod::tls()).unwrap();
        ctx.set_verify_callback(SSL_VERIFY_PEER, |_, _| true);
        //assert!(ctx.set_certificate_file(&path, X509_FILETYPE_PEM).is_ok());
        let ctx = ctx.build();
        let connector = MumbleConnector(ctx);
        connector.connect_async(mumble_server, socket)
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
        println!("Sending version: {:?}", version);
        tokio_io::io::write_all(stream, buf)

    }).and_then(|(stream, _)| { // Authenticate
        let mut auth = mumble::Authenticate::new();
        auth.set_username(format!("avatar-ue4-{}", 10000 + rand::random::<u16>()));
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

    }).and_then(move|(stream, _)| {

        let (mum_reader, mum_writer) = stream.split();

        // mumble writer
        let mum_writer = mum_rx.fold(mum_writer, move |writer, msg : Vec<u8>| {
            tokio_io::io::write_all(writer, msg)
            .map(|(writer, _)| writer)
            .map_err(|_| ())
        })
        .map_err(|_| Error::new(ErrorKind::Other, "writing to tcp"));

        let mumble_ping = mumble_ping(mum_tx.clone());
        let mumble_loop = mumble_loop(Box::new(RemoteSessions::new()),mum_reader, vox_tx);

        Future::join3(mumble_ping, mum_writer, mumble_loop)
    })
        .map(|_| ());

    (comm, mum_tx0, vox_rx)
}
