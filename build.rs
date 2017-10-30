use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() {
    println!("cargo:rustc-link-search=native={}", "C:\\gstreamer\\1.0\\x86_64\\lib");
}
