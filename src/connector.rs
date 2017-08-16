// #![allow(unused_imports)]
#![allow(dead_code)]

extern crate futures;
extern crate openssl;
extern crate tokio_io;

use std::io::{self, Read, Write};

use futures::{Poll, Future, Async};
use openssl::ssl::{self, Ssl, SslAcceptor, SslConnector, SslContext, Error, HandshakeError, ShutdownResult};
use tokio_io::{AsyncRead, AsyncWrite};
#[allow(deprecated)]
use tokio_core::io::Io;

#[derive(Debug)]
pub struct SslStream<S> {
    inner: ssl::SslStream<S>,
}

pub struct ConnectAsync<S> {
    inner: MidHandshake<S>,
}

pub struct AcceptAsync<S> {
    inner: MidHandshake<S>,
}

struct MidHandshake<S> {
    inner: Option<Result<ssl::SslStream<S>, HandshakeError<S>>>,
}

pub trait SslConnectorExt {
    fn connect_async<S>(&self, domain: &str, stream: S) -> ConnectAsync<S>
        where S: Read + Write;
}

pub trait SslAcceptorExt {
    fn accept_async<S>(&self, stream: S) -> AcceptAsync<S>
        where S: Read + Write;
}

impl<S> SslStream<S> {

    pub fn get_ref(&self) -> &ssl::SslStream<S> {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut ssl::SslStream<S> {
        &mut self.inner
    }
}

impl<S: Read + Write> Read for SslStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<S: Read + Write> Write for SslStream<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[allow(deprecated)]
impl<S: Io> Io for SslStream<S> {
}

impl<S: AsyncRead + AsyncWrite> AsyncRead for SslStream<S> {}

impl<S: AsyncRead + AsyncWrite> AsyncWrite for SslStream<S> {
    fn shutdown(&mut self) -> Poll<(), io::Error> {
        loop {
            match self.inner.shutdown() {
                Ok(ShutdownResult::Sent) => {},
                Ok(ShutdownResult::Received) => break,
                Err(ssl::Error::ZeroReturn) => break,
                Err(ssl::Error::Stream(e)) => return Err(e),
                Err(ssl::Error::WantRead(_e)) => return Ok(Async::NotReady),
                Err(ssl::Error::WantWrite(_e)) => return Ok(Async::NotReady),
                Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e)),
            }
        }

        Ok(Async::Ready(()))
    }
}

impl SslConnectorExt for SslConnector {
    fn connect_async<S>(&self, domain: &str, stream: S) -> ConnectAsync<S>
        where S: Read + Write,
    {
        ConnectAsync {
            inner: MidHandshake {
                inner: Some(self.connect(domain, stream)),
            },
        }
    }
}

impl SslAcceptorExt for SslAcceptor {
    fn accept_async<S>(&self, stream: S) -> AcceptAsync<S>
        where S: Read + Write,
    {
        AcceptAsync {
            inner: MidHandshake {
                inner: Some(self.accept(stream)),
            },
        }
    }
}

impl<S: Read + Write> Future for ConnectAsync<S> {
    type Item = SslStream<S>;
    type Error = Error;

    fn poll(&mut self) -> Poll<SslStream<S>, Error> {
        self.inner.poll()
    }
}

impl<S: Read + Write> Future for AcceptAsync<S> {
    type Item = SslStream<S>;
    type Error = Error;

    fn poll(&mut self) -> Poll<SslStream<S>, Error> {
        self.inner.poll()
    }
}

impl<S: Read + Write> Future for MidHandshake<S> {
    type Item = SslStream<S>;
    type Error = Error;

    fn poll(&mut self) -> Poll<SslStream<S>, Error> {
        match self.inner.take().expect("cannot poll MidHandshake twice") {
            Ok(stream) => Ok(SslStream { inner: stream }.into()),
            Err(HandshakeError::Failure(e)) => Err(e.into_error()),
            Err(HandshakeError::SetupFailure(e)) => Err(Error::Ssl(e)),
            Err(HandshakeError::Interrupted(s)) => {
                match s.handshake() {
                    Ok(stream) => Ok(SslStream { inner: stream }.into()),
                    Err(HandshakeError::Failure(e)) => Err(e.into_error()),
                    Err(HandshakeError::SetupFailure(e)) => Err(Error::Ssl(e)),
                    Err(HandshakeError::Interrupted(s)) => {
                        self.inner = Some(Err(HandshakeError::Interrupted(s)));
                        Ok(Async::NotReady)
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct MumbleConnector(pub SslContext);

impl MumbleConnector {

    pub fn connect<S>(&self, domain: &str, stream: S) -> Result<ssl::SslStream<S>, HandshakeError<S>>
        where S: Read + Write
    {
        let mut ssl = try!(Ssl::new(&self.0));
        try!(ssl.set_hostname(domain));
        // try!(setup_verify(&mut ssl, domain));
        ssl.connect(stream)
    }

    pub fn connect_async<S>(&self, domain: &str, stream: S) -> ConnectAsync<S>
        where S: Read + Write,
    {
        ConnectAsync {
            inner: MidHandshake {
                inner: Some(self.connect(domain, stream)),
            },
        }
    }
}
