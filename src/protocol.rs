use std;
use std::io;
// use std::str;
use bytes::BytesMut;
use tokio_io::codec::{Encoder, Decoder};

#[derive(Debug)]
struct Error;

impl std::error::Error for Error {
    fn description(&self) -> &str {
        "Something bad happened"
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Oh no, something bad went down")
    }
}

// trait Protocol {
//     fn add_10<F>(f: F) -> impl Future<Item = i32, Error = F::Error>
//         where
//             F: Future<Item = i32>
// }

pub struct Codec;

impl Decoder for Codec {
    type Item = String;
    type Error = Error;

}

impl Encoder for Codec {
    type Item = String;
    type Error = Error;

}
