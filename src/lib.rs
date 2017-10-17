#![allow(dead_code)]
#![allow(unused_imports)]
#![feature(conservative_impl_trait)]

extern crate pretty_env_logger;
#[macro_use]
extern crate log;
extern crate clap;

extern crate openssl;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_timer;
extern crate protobuf;
extern crate byteorder;
extern crate rand;
extern crate opus;
extern crate chrono;
extern crate ocbaes128;

extern crate toml;
#[macro_use]
extern crate serde_derive;

use clap::{Arg, App};
extern crate futures;

use std::fs;

use std::io::Cursor;
use std::io::{Write, Read, Error, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::{Sink, Stream};
use futures::future::{Future, ok, loop_fn, IntoFuture, Loop};

use openssl::ssl::{SslContext, SslMethod, SSL_VERIFY_PEER};
use openssl::x509::X509_FILETYPE_PEM;

use tokio_io::AsyncRead;
use tokio_core::net::{TcpStream, UdpSocket, UdpCodec};
use tokio_core::reactor::{Core, Timeout};

use protobuf::Message;
use protobuf::{CodedOutputStream, CodedInputStream};

use byteorder::{BigEndian, LittleEndian, ReadBytesExt, WriteBytesExt};

mod udp;
mod tcp;

mod mumble;
mod connector;
use connector::MumbleConnector;

mod session;

mod varint;
use varint::VarintReader;
use varint::VarintWriter;

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

pub fn say<'a>(vox_out_rx: futures::sync::mpsc::Receiver<Vec<u8>>,
               udp_tx: futures::sync::mpsc::Sender<Vec<u8>>,
               crypt_state: Arc<Mutex<ocbaes128::CryptState>>)
               -> impl Future<Item = (), Error = Error> + 'a {

    // Hz * channel * ms / 1000
    let sample_channels: u32 = 1;
    let sample_rate: u32 = 16000;
    let sample_ms: u32 = 10;
    let sample_size: u32 = sample_rate * sample_channels * sample_ms / 1000;

    println!("sample channels: {} rate: {} ms: {} size: {}",
             sample_channels,
             sample_rate,
             sample_ms,
             sample_size);

    let mut encoder =
        opus::Encoder::new(sample_rate, opus::Channels::Mono, opus::Application::Voip).unwrap();

    let mut sequence = chrono::UTC::now().timestamp() as u64;

    vox_out_rx.map(|segment| futures::stream::iter_ok::<_, ()>(segment))
        .flatten()
        .chunks(2)
        .and_then(|raw| {
            let pcm = (&raw[..]).read_i16::<LittleEndian>().unwrap();
            ok::<i16, ()>(pcm)
        })
        .chunks(sample_size as usize)
        .fold(udp_tx, move |udp_tx, chunk| {
            let mut chunk = Vec::from(chunk);
            chunk.resize(sample_size as usize, 0);
            let frame = encoder.encode_vec(&chunk[..], 4000).unwrap();

            sequence = sequence + 1;

            let done = false;

            let aud_header = 0b100 << 5;
            let mut data = Vec::<u8>::new();
            data.write_u8(aud_header).unwrap();
            data.write_varint(sequence).unwrap();
            let opus_len = if done {
                frame.len() as u64 | 0x2000
            } else {
                frame.len() as u64
            };
            data.write_varint(opus_len).unwrap();
            data.write_all(&frame).unwrap();

            let mut buf = vec![0u8; data.len() + 4];
            {
                let mut crypt_state = crypt_state.lock().unwrap();
                crypt_state.encrypt(&data[..], &mut buf[..])
            };

            udp_tx.send(buf)
                .map_err(|_| ())
        })
        .map(|_| ())
        .map_err(|_| Error::new(ErrorKind::Other, "vox out"))
}

pub fn say_test(vox_out_tx: futures::sync::mpsc::Sender<Vec<u8>>) {

    std::thread::sleep(std::time::Duration::from_secs(5));

    // // Hz * channel * ms / 1000
    let sample_channels: u32 = 1;
    let sample_rate: u32 = 16000;
    let sample_ms: u32 = 10;
    let sample_size: u32 = sample_rate * sample_channels * sample_ms / 1000;

    let mut pcm_file = fs::File::open("data/man16kHz.raw").unwrap();
    let mut pcm_data = Vec::<u8>::new();
    pcm_file.read_to_end(&mut pcm_data).unwrap();

    let mut vox_out_tx = vox_out_tx;
    for buf in pcm_data.chunks(sample_size as usize * 2) {
        std::thread::sleep(std::time::Duration::from_millis((sample_ms as f32 / 1.1) as u64));
        let tx0 = vox_out_tx.send(Vec::from(&buf[..]));
        vox_out_tx = tx0.wait().unwrap();
    }
}

pub fn udp_crypt() -> Arc<Mutex<ocbaes128::CryptState>> {
    let crypt_state = ocbaes128::CryptState::new();
    Arc::new(Mutex::new(crypt_state))
}

pub fn cmd() {
    pretty_env_logger::init().unwrap();

    let matches = app().get_matches();

    let config: config::Config = {
        let config_file = matches.value_of("cfg").unwrap_or("data/Config.toml");
        let mut config_file = fs::File::open(config_file).unwrap();
        let mut config = String::new();
        config_file.read_to_string(&mut config).unwrap();
        toml::from_str(&config).unwrap()
    };

    let local_addr: SocketAddr = String::from("192.168.0.39:0").parse().unwrap();
    let mumble_server: SocketAddr = config.mumble.server.parse().unwrap();
    let mut core = Core::new().unwrap();
    let handle = core.handle();

    let crypt_state = udp_crypt();

    let (vox_out_tx, vox_out_rx) = futures::sync::mpsc::channel::<Vec<u8>>(1000);
    let (vox_inp_tx, vox_inp_rx) = futures::sync::mpsc::channel::<Vec<u8>>(1000);
    let vox_inp_task = vox_inp_rx.fold((), |_, _| {
        ok::<(), ()>(())
    })
    .map_err(|_| Error::new(ErrorKind::Other, "vox_inp_task"));

    let (app_logic, _tcp_tx, udp_tx) = run(local_addr, mumble_server, vox_inp_tx.clone(), Arc::clone(&crypt_state), &handle);

    let say_task = say(vox_out_rx, udp_tx.clone(), Arc::clone(&crypt_state));

    std::thread::spawn(move || {
        say_test(vox_out_tx.clone());
    });

    let tasks = Future::join3(app_logic, say_task, vox_inp_task);
    core.run(tasks).unwrap();
}

pub fn run<'a>(local_addr: SocketAddr, mumble_server: SocketAddr,
               vox_inp_tx: futures::sync::mpsc::Sender<Vec<u8>>,
               crypt_state: Arc<Mutex<ocbaes128::CryptState>>,
               handle: &tokio_core::reactor::Handle)
               -> (impl Future<Item = (), Error = Error> + 'a,
                   futures::sync::mpsc::Sender<Vec<u8>>,
                   futures::sync::mpsc::Sender<Vec<u8>>) {

    let udp_server_addr: SocketAddr = mumble_server;
    let udp_local_addr: SocketAddr = local_addr;
    let udp_socket = UdpSocket::bind(&udp_local_addr, &handle).unwrap();
    let (udp_socket_tx, udp_socket_rx) = udp_socket.framed(udp::AudioPacker).split();
    let (udp_tx, udp_rx) = futures::sync::mpsc::channel::<Vec<u8>>(0);
    let udp_tx0 = udp_tx.clone();

    let tcp_server_addr: SocketAddr = mumble_server;
    let (tcp_tx, tcp_rx) = futures::sync::mpsc::channel::<Vec<u8>>(1000);
    let tcp_tx0 = tcp_tx.clone();

    let comm = TcpStream::connect(&tcp_server_addr, &handle).and_then(move|socket| {
        println!("Connecting to mumble_server: {}", mumble_server);
//        let path = Path::new("mumble.pem");
        let mut ctx = SslContext::builder(SslMethod::tls()).unwrap();
        ctx.set_verify_callback(SSL_VERIFY_PEER, |_, _| true);
        //assert!(ctx.set_certificate_file(&path, X509_FILETYPE_PEM).is_ok());
        let ctx = ctx.build();
        let connector = MumbleConnector(ctx);
        connector.connect_async(&format!("{}:{}", mumble_server.ip(), mumble_server.port()), socket)
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
        auth.set_username(format!("mumbot-{}", (10000 as usize).wrapping_add(rand::random::<usize>())));
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

        let (tcp_socket_rx, tcp_socket_tx) = stream.split();
        let tcp_writer = tcp_rx.fold(tcp_socket_tx, move |writer, msg : Vec<u8>| {
            tokio_io::io::write_all(writer, msg)
            .map(|(writer, _)| writer)
            .map_err(|_| ())
        })
        .map_err(|_| Error::new(ErrorKind::Other, "writing to tcp"));

        let udp_writer = udp_rx.fold(udp_socket_tx, move |tx, msg| {
            tx.send((udp_server_addr, msg))
            .map_err(|_| ())
        })
        .map_err(|_| Error::new(ErrorKind::Other, "writing to udp"));

        let tcp_ping = tcp::tcp_ping(tcp_tx.clone());
        let udp_ping = udp::udp_ping(udp_tx.clone(), Arc::clone(&crypt_state));

        let remotes = Arc::new(Mutex::new(session::Remotes::new()));

        let tcp_recv_loop = tcp::tcp_recv_loop(Arc::clone(&remotes), tcp_socket_rx, tcp_tx, vox_inp_tx.clone(), Arc::clone(&crypt_state));
        let udp_recv_loop = udp::udp_recv_loop(Arc::clone(&remotes), udp_socket_rx, udp_tx, vox_inp_tx.clone(), Arc::clone(&crypt_state));

        let send_tasks = Future::join(tcp_writer, udp_writer);
        let ping_tasks = Future::join(tcp_ping, udp_ping);
        let recv_tasks = Future::join(tcp_recv_loop, udp_recv_loop);

        Future::join3(ping_tasks, recv_tasks, send_tasks)
    })
    .map(|_| ());

    (comm, tcp_tx0, udp_tx0)
}