use hyper::{Client, Request, Method};
use hyper::client::HttpConnector;
use hyper::header;
use hyper::mime;
use hyper_tls::{HttpsConnector};
use chrono;
use tokio_core;
use warheadhateus::{AWSAuth, HttpRequestMethod, Region, Service};
use config;

pub fn request (config : &config::Config, user_id : u32) -> Request {

    let access_key_id = &config.aws.access_key_id;
    let secret_access_key = &config.aws.secret_access_key;

    let user_id = format!("{:05}", user_id);
    let post = format!("/bot/Lyric/alias/Lyric/user/{}/content", user_id);
    trace!("user_id {}", user_id);
    let host = format!("runtime.lex.us-east-1.amazonaws.com");
    let url = format!("https://{}{}", host, post);
    let uri = url.parse().unwrap();

    let now = chrono::UTC::now().format("%Y%m%dT%H%M%SZ").to_string();

    let mut auth = AWSAuth::new(&url).unwrap();
    let payload_hash = "UNSIGNED-PAYLOAD";

    auth.set_request_type(HttpRequestMethod::POST)
        .set_payload_hash(&payload_hash)
        .set_service(Service::LEX)
        .set_access_key_id(access_key_id)
        .set_secret_access_key(secret_access_key)
        .set_region(Region::UsEast1)
        .add_header("host", &host)
        .add_header("content-type", "audio/l16; rate=16000; channels=1")
        // .add_header("content-type", "audio/x-cbr-opus-with-preamble; preamble-size=0; bit-rate=256000; frame-size-milliseconds=4")
        .add_header("x-amz-content-sha256", &payload_hash)
        .add_header("x-amz-date", &now);
    let ah = auth.auth_header().unwrap();

    let mut req = Request::new(Method::Post, uri);
    req.headers_mut().set(header::ContentType("audio/l16; rate=16000; channels=1".parse::<mime::Mime>().unwrap()));
    req.headers_mut().set(header::Host::new(host, None));
    req.headers_mut().set(header::Authorization(ah));
    req.headers_mut().set_raw("accept", "audio/pcm");
    req.headers_mut().set_raw("x-amz-date", now);
    req.headers_mut().set_raw("x-amz-content-sha256", payload_hash);

    req
}

pub fn client (handle : &tokio_core::reactor::Handle) -> Client<HttpsConnector<HttpConnector>> {
    Client::configure()
        .connector(HttpsConnector::new(4, &handle).unwrap())
        .build(&handle)
}
