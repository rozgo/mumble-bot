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
use opus;

use byteorder;
use byteorder::{BigEndian, LittleEndian, ReadBytesExt, WriteBytesExt};
use varint::VarintReader;
use varint::VarintWriter;

use session;
use util;

pub struct AudioOutPacket {
    pub type_: u32,
    pub target: u32,
    pub pcm: Vec<i16>,
    pub done: bool,
    pub timestamp: u64,
}

pub struct AudioInPacket {
    pub type_: u32,
    pub target: u32,
    pub pcm: Vec<u8>,
}

pub struct AudioPacketCodec {
    pub opus_encoder: opus::Encoder,
    pub opus_decoder: opus::Decoder,
    pub crypt_state: Arc<Mutex<ocbaes128::CryptState>>,
    pub encoder_sequence: u64,
    pub decoder_sequence: u64,
}

impl UdpCodec for AudioPacketCodec {
    type In = (SocketAddr, AudioInPacket);
    type Out = (SocketAddr, AudioOutPacket);

    fn decode(&mut self, addr: &SocketAddr, buf: &[u8]) -> std::io::Result<Self::In> {
        let crypt_len =  buf.len();
        println!("crypt_len: {}", crypt_len);
        let mut data = vec![0u8; crypt_len - 4];
        {
            let decrypt = self.crypt_state.lock().unwrap().decrypt(&buf, &mut data);
            println!("decrypt: {}", decrypt);
        }

        let mut rdr = Cursor::new(&data);
        let aud_header = rdr.read_u8().unwrap();
        // println!("incoming aud_header: {}", aud_header);
        let aud_type = (aud_header & 0b11100000) >> 5;
        let aud_target = aud_header & 0b00011111;

        let data = match aud_type {
            0b100 => { // OPUS encoded voice data
                let _aud_session = rdr.read_varint().unwrap();
                let sequence = rdr.read_varint().unwrap();
                println!("audio packet type: OPUS target: {} sequence: {}", aud_target, sequence);
                self.decoder_sequence = sequence;
                let mut opus_frame = Vec::<u8>::new();
                rdr.read_to_end(&mut opus_frame).unwrap();
                let (data, _done) = util::opus_decode(&mut self.opus_decoder, opus_frame);
                data
            },
            _ => vec![],
        };

        Ok((*addr, AudioInPacket{type_: aud_type as u32, target: aud_target as u32, pcm: data}))
    }

    fn encode(&mut self, (addr, packet): Self::Out, into: &mut Vec<u8>) -> SocketAddr {
        let mut data = vec![0u8; 0];
        match packet.type_ {
            0b001 => {
                data.push(0b00100000);
                data.write_varint(packet.timestamp).unwrap();
            },
            0b100 => {
                let frame = self.opus_encoder.encode_vec(&packet.pcm, 4000).unwrap();
                self.encoder_sequence = self.encoder_sequence + 1;
                let done = false;
                let aud_header = 0b100 << 5;
                data.write_u8(aud_header).unwrap();
                data.write_varint(self.encoder_sequence).unwrap();
                let opus_len = if done {
                    frame.len() as u64 | 0x2000
                } else {
                    frame.len() as u64
                };
                data.write_varint(opus_len).unwrap();
                data.write_all(&frame).unwrap();
            },
            _ => panic!("AudioPacketCodec:encode type unknown")
        }
        let mut enc = vec![0u8; data.len() + 4];
        self.crypt_state.lock().unwrap().encrypt(&data, &mut enc);
        into.extend(enc);
        addr
    }
}

pub fn udp_recv_loop<'a>(
    udp_socket_rx: futures::stream::SplitStream<tokio_core::net::UdpFramed<AudioPacketCodec>>,
    _udp_tx: futures::sync::mpsc::Sender<AudioOutPacket>,
    vox_inp_tx: futures::sync::mpsc::Sender<Vec<u8>>)
    -> impl Future<Item = (), Error = Error> + 'a {

    udp_socket_rx.fold(vox_inp_tx, move |vox_inp_tx, (_socket, packet)| {

        match packet.type_ {

            0b100 => { // OPUS encoded voice data
                println!("audio packet type: OPUS target: {}", packet.target);
                vox_inp_tx.send(packet.pcm)
                .and_then(move |vox_inp_tx| {
                    ok(vox_inp_tx)
                })
                .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
                .boxed()
            },
            0b001 => { // Ping
                println!("audio packet type: Ping target: {}", packet.target);
                ok(vox_inp_tx)
                .boxed()
            },
            0b000 => { // CELT Alpha
                println!("audio packet type: CELT Alpha target: {}", packet.target);
                ok(vox_inp_tx)
                .boxed()
            },
            0b010 => { // Speex
                println!("audio packet type: Speex target: {}", packet.target);
                ok(vox_inp_tx)
                .boxed()
            },
            0b111 => { // dropped
                println!("audio packet type: DROPPED target: {}", packet.target);
                ok(vox_inp_tx)
                .boxed()
            },
            _ => {
                println!("audio packet unknown type: {:b} target: {}", packet.type_, packet.target);
                ok(vox_inp_tx)
                .boxed()
            }
        }
        .map_err(|e: Error| Error::new(ErrorKind::Other, e.to_string()))
    })
    .map(|_| ())
    .map_err(|_| Error::new(ErrorKind::Other, "udp_loop"))
}

pub fn udp_ping(udp_tx: futures::sync::mpsc::Sender<AudioOutPacket>)
    -> impl Future<Item = (), Error = Error> {
    tokio_timer::Timer::default()
        .interval(Duration::from_secs(5))
        .fold(udp_tx, move |tx, _| {
            let packet = AudioOutPacket {
                type_: 0b001,
                target: 0,
                pcm: vec![],
                done: false,
                timestamp: chrono::UTC::now().timestamp() as u64,
            };
            tx.send(packet)
                .map_err(|_| tokio_timer::TimerError::NoCapacity)
        })
        .map(|_| ())
        .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
}

