extern crate mumblebot;
extern crate pretty_env_logger;

use mumblebot::*;

pub fn main() {
    pretty_env_logger::init().unwrap();

    for i in 0..10 {
        println!("%%%%  ----  SESSION: {}  ----  %%%%", i);
        cmd();
    }
}
