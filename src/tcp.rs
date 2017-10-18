use std;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::io::{Cursor, Write, Read, Error, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use futures;
use futures::{Sink, Stream};
use futures::future::{Future, ok, loop_fn, IntoFuture, Loop};

use tokio_core;
use tokio_core::net::{UdpSocket, UdpCodec};
use tokio_io;
use tokio_timer;

use chrono;
use ocbaes128;
use connector;
use protobuf::Message;
use protobuf::{CodedOutputStream, CodedInputStream};

use byteorder;
use byteorder::{BigEndian, LittleEndian, ReadBytesExt, WriteBytesExt};
use varint::VarintReader;
use varint::VarintWriter;

use mumble;
use session;
use util;


pub fn tcp_recv_loop<'a>(
                tcp_socket_rx: tokio_io::io::ReadHalf<connector::SslStream<tokio_core::net::TcpStream>>,
                tcp_tx: futures::sync::mpsc::Sender<std::vec::Vec<u8>>,
                vox_inp_tx: futures::sync::mpsc::Sender<Vec<u8>>,
                crypt_state: Arc<Mutex<ocbaes128::CryptState>>)
                -> impl Future<Item = (), Error = Error> + 'a {

    loop_fn((tcp_socket_rx, tcp_tx, vox_inp_tx, session::Local {session: 0}, session::Remotes::new(), crypt_state),
            |(tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)| {

        // get new packet
        tokio_io::io::read_exact(tcp_socket_rx, [0u8; 2])
        .and_then(|(tcp_socket_rx, buf)| {
            let mut rdr = Cursor::new(&buf);
            let mum_type = rdr.read_u16::<BigEndian>().unwrap();
            trace!("mum_type: {}", mum_type);
            tokio_io::io::read_exact(tcp_socket_rx, [0u8; 4])
            .and_then(move |(rx, buf)| {
                ok((rx, buf, mum_type))
            })
        })

        // packet length
        .and_then(|(tcp_socket_rx, buf, mum_type)| {
            let mut rdr = Cursor::new(&buf);
            let mum_length = rdr.read_u32::<BigEndian>().unwrap();
            tokio_io::io::read_exact(tcp_socket_rx, vec![0u8; mum_length as usize])
            .and_then(move |(rx, buf)| ok((rx, buf, mum_type)))
        })

        // packet payload
        .and_then(|(tcp_socket_rx, buf, mum_type)| {

            println!("mum_type: {}", mum_type);

            let mut inp = CodedInputStream::from_bytes(&buf);

            match mum_type {
                0 => { // Version
                    let mut msg = mumble::Version::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("version: {:?}", msg);
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
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
                            ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                        },
                        32 => { // Ping
                            ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                        },
                        _ => {
                            panic!("aud_type");
                        }
                    }
                },
                3 => { // Ping
                    let mut msg = mumble::Ping::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("TCP Ping: {:?}", msg);
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                },
                5 => { // ServerSync
                    let mut msg = mumble::ServerSync::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("ServerSync: {:?}", msg);
                    let local_session = session::Local {session: msg.get_session() as u64};
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                },
                7 => { // ChannelState
                    let mut msg = mumble::ChannelState::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("ChannelState: {:?}", msg);
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                },
                8 => { // UserRemove
                    let mut msg = mumble::UserRemove::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("UserRemove: {:?}", msg);
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                },
                9 => { // UserState
                    let mut msg = mumble::UserState::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("UserState: {:?}", msg);
                    let remote_sessions = session::factory(msg.get_session() as u64, remote_sessions);
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                },
                11 => { // TextMessage
                    let mut msg = mumble::TextMessage::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("TextMessage: {:?}", msg);
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                },
                15 => { // CryptSetup
                    let mut msg = mumble::CryptSetup::new();
                    msg.merge_from(&mut inp).unwrap();
                    let crypt_key = msg.get_key();
                    let crypt_client_nonce = msg.get_client_nonce();
                    let crypt_server_nonce = msg.get_server_nonce();
                    {
                        let mut crypt_state = crypt_state.lock().unwrap();
                        crypt_state.set_key(crypt_key, crypt_client_nonce, crypt_server_nonce);
                    }
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                },
                20 => { // PermissionQuery
                    let mut msg = mumble::PermissionQuery::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("PermissionQuery: {:?}", msg);
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                },
                21 => { // CodecVersion
                    let mut msg = mumble::CodecVersion::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("CodecVersion: {:?}", msg);
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                },
                24 => { // ServerConfig
                    let mut msg = mumble::ServerConfig::new();
                    msg.merge_from(&mut inp).unwrap();
                    println!("ServerConfig: {:?}", msg);
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                },
                _ => {
                    ok((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)).boxed()
                }
            }
        })
        .and_then(move |(tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)| {
            Ok(Loop::Continue((tcp_socket_rx, tcp_tx, vox_inp_tx, local_session, remote_sessions, crypt_state)))
        })
    })
}

pub fn tcp_ping(mum_tx: futures::sync::mpsc::Sender<Vec<u8>>) -> impl Future<Item = (), Error = Error> {
    tokio_timer::Timer::default()
        .interval(Duration::from_secs(5))
        .fold(mum_tx, move |tx, _| {
            let timestamp = chrono::UTC::now().timestamp() as u64;
            let mut ping = mumble::Ping::new();
            ping.set_timestamp(timestamp);
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
