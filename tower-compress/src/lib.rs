extern crate deflate;
#[macro_use]
extern crate futures;
extern crate http;
extern crate tower_service;

use deflate::write::{DeflateEncoder, GzEncoder};
use futures::{Async, Future, Poll};
use http::{Request, Response};
use http::header::{self, HeaderValue};
use tower_service::Service;

use std::io::{self, Write};

/// A service that compresses the response of the wrapped service.
#[derive(Clone, Debug)]
pub struct Compress<T> {
    inner: T,
    options: deflate::CompressionOptions,
}

#[derive(Clone, Debug)]
pub struct CompressFuture<T> {
    inner: T,
    encoding: Encoding,
    options: deflate::CompressionOptions,
}

/// Constructs instances of `Deflate`.
#[derive(Clone, Debug, Default)]
pub struct Builder {
    options: deflate::CompressionOptions,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum Encoding {
    Deflate,
    Gzip,
    Uncompressed,
    // TODO: add support for `accept-encoding: brotli`,
    //       and `accept-encoding: compress`.
}

enum Encoder<W: Write> {
    Deflate(DeflateEncoder<W>),
    Gzip(GzEncoder<W>),
    Uncompressed(W),
}

#[derive(Debug)]
pub enum Error<T> {
    Inner(T),
    Write(io::Error),
    Finish(io::Error),
}

// ===== impl Compress =====

impl<T> Compress<T> {
    pub fn new(inner: T) -> Self {
        Compress {
            inner,
            options: deflate::CompressionOptions::default(),
        }
    }

    /// Returns a reference to the inner service.
    pub fn get_ref(&self) -> &T {
        &self.inner
    }

    /// Returns a mutable reference to the inner service.
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Consumes `self`, returning the inner service.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T, A, B> Service for Compress<T>
where
    T: Service<
        Request = Request<A>,
        Response = Response<B>
    >,
    B: AsRef<[u8]>,
{
    type Request = T::Request;
    type Response = Response<Vec<u8>>;
    type Error = Error<T::Error>;
    type Future = CompressFuture<T::Future>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready().map_err(Error::Inner)
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        let encoding = Encoding::from_request(&req);
        CompressFuture {
            inner: self.inner.call(req),
            options: self.options,
            encoding,
        }
    }
}

impl<T> CompressFuture<T> {
    fn make_encoder(&self, capacity: usize) -> Encoder<Vec<u8>> {
        use Encoding::*;
        let writer = Vec::<u8>::with_capacity(capacity);

        match self.encoding {
            Gzip => Encoder::Gzip(GzEncoder::new(writer, self.options)),
            Deflate => Encoder::Deflate(DeflateEncoder::new(writer, self.options)),
            Uncompressed => Encoder::Uncompressed(writer),
        }
    }
}

impl<T, B> Future for CompressFuture<T>
where
    T: Future<Item = Response<B>>,
    B: AsRef<[u8]>,
{
    type Item = Response<Vec<u8>>;
    type Error = Error<T::Error>;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let resp = try_ready!(self.inner.poll().map_err(Error::Inner));
        let (mut parts, body) = resp.into_parts();
        let body = body.as_ref();
        let capacity = if self.encoding.is_compressed() {
            parts.headers.insert(
                header::CONTENT_ENCODING,
                self.encoding.header_value(),
            );
            body.len() / 3
        } else {
            body.len()
        };
        let mut encoder = self.make_encoder(capacity);
        encoder.write(body).map_err(Error::Write)?;
        let body = encoder.finish().map_err(Error::Finish)?;
        Ok(Async::Ready(Response::from_parts(parts, body)))
    }
}


// ===== impl Encoding =====

impl Encoding {
    fn from_request<B>(req: &Request<B>) -> Self {
        // TODO: honor quality-items if present (rather than choosing
        // based on ordering)
        req.headers().get_all(header::ACCEPT_ENCODING).iter()
            .filter_map(|value| {
                value.to_str().ok().and_then(|value|
                    if value.contains("gzip") {
                        Some(Encoding::Gzip)
                    } else if value.contains("deflate") {
                        Some(Encoding::Deflate)
                    } else {
                        None
                    })
            })
            .next()
            .unwrap_or(Encoding::Uncompressed)
    }

    fn is_compressed(&self) -> bool {
        match *self {
            Encoding::Uncompressed => false,
            _ => true,
        }
    }

    fn header_value(&self) -> HeaderValue {
        match *self {
            Encoding::Deflate => HeaderValue::from_static("deflate"),
            Encoding::Gzip => HeaderValue::from_static("gzip"),
            Encoding::Uncompressed => HeaderValue::from_static("identity"),
        }

    }

}


// ===== impl Encoder =====

impl<W: Write> Encoder<W> {
    pub fn finish(self) -> io::Result<W> {
        match self {
            Encoder::Deflate(e) => e.finish(),
            Encoder::Gzip(e) => e.finish(),
            Encoder::Uncompressed(mut e) => {
                e.flush()?;
                Ok(e)
            },
        }
    }
}

impl<W: Write> Write for Encoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match *self {
            Encoder::Deflate(ref mut e) => e.write(buf),
            Encoder::Gzip(ref mut e) => e.write(buf),
            Encoder::Uncompressed(ref mut e) => e.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match *self {
            Encoder::Deflate(ref mut e) => e.flush(),
            Encoder::Gzip(ref mut e) => e.flush(),
            Encoder::Uncompressed(ref mut e) => e.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    mod encoding {
        use super::super::*;
        use http::Request;

        #[test]
        fn identity_is_none() {
            let req = Request::builder()
                .header("Accept-Encoding", "Identity")
                .body(())
                .unwrap();
            assert_eq!(Encoding::from_request(&req), Encoding::Uncompressed)
        }

        #[test]
        fn no_accept_encoding_is_none() {
            let req = Request::builder()
                .body(())
                .unwrap();
            assert_eq!(Encoding::from_request(&req), Encoding::Uncompressed)
        }

        #[test]
        fn unrecognizable_is_none() {
            let req = Request::builder()
                .header("Accept-Encoding", "inflate")
                .body(())
                .unwrap();
            assert_eq!(Encoding::from_request(&req), Encoding::Uncompressed)
        }

        #[test]
        fn gzip_recognized() {
            let req = Request::builder()
                .header("Accept-Encoding", "gzip")
                .body(())
                .unwrap();
            assert_eq!(Encoding::from_request(&req), Encoding::Gzip)
        }

        #[test]
        fn deflate_recognized() {
            let req = Request::builder()
                .header("Accept-Encoding", "deflate")
                .body(())
                .unwrap();
            assert_eq!(Encoding::from_request(&req), Encoding::Deflate)
        }

        #[test]
        fn picks_first_encoding() {
            let req = Request::builder()
                .header("Accept-Encoding", "gzip")
                .header("Accept-Encoding", "deflate")
                .body(())
                .unwrap();
            assert_eq!(Encoding::from_request(&req), Encoding::Gzip)
        }

        #[test]
        fn picks_first_recognizable_compressed_encoding() {
            let req = Request::builder()
                .header("Accept-Encoding", "inflate")
                .header("Accept-Encoding", "gzip")
                .header("Accept-Encoding", "deflate")
                .body(())
                .unwrap();
            assert_eq!(Encoding::from_request(&req), Encoding::Gzip)
        }
    }

}
