use std;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::io::{Cursor, Write, Read, Error, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use futures;
use futures::{Sink, Stream};
use futures::future::{Future, ok, loop_fn, IntoFuture, Loop};

use tokio_core;
use tokio_core::net::{UdpSocket, UdpCodec};
use tokio_timer;
use chrono;
use std::time::Duration;
use ocbaes128;

use byteorder;
use byteorder::{BigEndian, LittleEndian, ReadBytesExt, WriteBytesExt};
use varint::VarintReader;
use varint::VarintWriter;

use session;
use util;


pub struct AudioPacker;

impl UdpCodec for AudioPacker {
    type In = (SocketAddr, Vec<u8>);
    type Out = (SocketAddr, Vec<u8>);

    fn decode(&mut self, addr: &SocketAddr, buf: &[u8]) -> std::io::Result<Self::In> {
        Ok((*addr, buf.to_vec()))
    }

    fn encode(&mut self, (addr, buf): Self::Out, into: &mut Vec<u8>) -> SocketAddr {
        into.extend(buf);
        addr
    }
}

pub fn udp_recv_loop<'a>(remote_sessions: Arc<Mutex<HashMap<u64, std::boxed::Box<session::Remote>>>>,
    udp_socket_rx: futures::stream::SplitStream<tokio_core::net::UdpFramed<AudioPacker>>,
    _udp_tx: futures::sync::mpsc::Sender<std::vec::Vec<u8>>,
    vox_inp_tx: futures::sync::mpsc::Sender<Vec<u8>>,
    crypt_state: Arc<Mutex<ocbaes128::CryptState>>)
    -> impl Future<Item = (), Error = Error> + 'a {

    udp_socket_rx.fold((remote_sessions, vox_inp_tx), move |(remote_sessions, vox_inp_tx), (_socket, msg)| {

        let mut rdr = Cursor::new(&msg);
        let aud_header = rdr.read_u8().unwrap();
        // println!("incoming aud_header: {}", aud_header);
        let aud_type = aud_header & 0b11100000;
        let aud_target = aud_header & 0b00011111;
        println!("type: {} target: {}", aud_type, aud_target);

        match aud_type {

            128 => { // OPUS encoded voice data

                let aud_session = rdr.read_varint().unwrap();
                let _aud_sequence = rdr.read_varint().unwrap();
                let remote_sessions = session::factory(aud_session, remote_sessions);

                let mut opus_frame = Vec::<u8>::new();
                rdr.read_to_end(&mut opus_frame).unwrap();

                let crypt_len =  opus_frame.len();
                // let mut data = if crypt_len > 4 {
                //     let mut data = vec![0u8; crypt_len - 4];
                //     let mut crypt_state = crypt_state.lock().unwrap();
                //     if crypt_state.decrypt(&opus_frame[..], &mut data) {
                //         data
                //     }
                //     else {
                //         vec![0u8; 12]
                //     }
                // }
                // else {
                //     vec![0u8; 12]
                // };

                let mut data = vec![0u8; crypt_len - 4];
                let mut crypt_state = crypt_state.lock().unwrap();
                crypt_state.decrypt(&opus_frame[..], &mut data);

                let (pcm, _done) = {
                    let mut remote_sessions = remote_sessions.lock().unwrap();
                    let remote_session = remote_sessions.get_mut(&aud_session).unwrap();
                    util::opus_decode(remote_session, data)
                };

                vox_inp_tx.send(pcm)
                .and_then(move |vox_inp_tx| {
                    ok((remote_sessions, vox_inp_tx))
                })
                .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
                .boxed()
            },
            32 => { // Ping
                ok((remote_sessions, vox_inp_tx))
                .boxed()
            },
            _ => {
                ok((remote_sessions, vox_inp_tx))
                .boxed()
            }
        }
    })
    .map(|_| ())
    .map_err(|_| Error::new(ErrorKind::Other, "udp_loop"))
}

pub fn udp_ping(mum_tx: futures::sync::mpsc::Sender<Vec<u8>>, crypt_state: Arc<Mutex<ocbaes128::CryptState>>)
    -> impl Future<Item = (), Error = Error> {

    tokio_timer::Timer::default()
        .interval(Duration::from_secs(5))
        .fold(mum_tx, move |tx, _| {

            let timestamp = chrono::UTC::now().timestamp() as u64;

            let mut data = Vec::new();
            data.push(0b00100000);
            data.write_varint(timestamp).unwrap();
            let mut buf = vec![0u8; data.len() + 4];
            {
                let mut crypt_state = crypt_state.lock().unwrap();
                crypt_state.encrypt(&data[..], &mut buf[..])
            };

            tx.send(buf)
                .map_err(|_| tokio_timer::TimerError::NoCapacity)
        })
        .map(|_| ())
        .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
}

