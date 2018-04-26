extern crate flate2;
extern crate futures;
extern crate http;
extern crate tower_compress;
extern crate tower_mock;
extern crate tower_service;

use flate2::read;
use futures::future::Future;
use http::{header, Request, Response};
use tower_compress::*;
use tower_mock::*;
use tower_service::Service;

use std::io::{Cursor, Read};

#[test]
fn deflates_requests(){
    let (mock, mut handle) = Mock::<_, _, ()>::new();
    let mut compress = Compress::new(mock);

    let request = Request::get("/")
        .header("Accept-Encoding", "deflate")
        .body(())
        .unwrap();

    let response_future = compress.call(request);

    let (_request, send_response) = handle.next_request()
        .unwrap()
        .into_parts();

    send_response.respond(Response::builder()
        .status(200)
        .body(b"hello deflated world!")
        .expect("send response"));

    let response = response_future.wait()
        .expect("response future");

    assert!(response.headers()
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .any(|v| v == "deflate")
    );

    let body_reader = Cursor::new(response.into_body());
    let mut decoder = read::DeflateDecoder::new(body_reader);
    let mut decompressed_body = String::new();
    decoder.read_to_string(&mut decompressed_body)
        .expect("decompress");

    assert_eq!("hello deflated world!", &decompressed_body)
}

#[test]
fn gzips_requests(){
    let (mock, mut handle) = Mock::<_, _, ()>::new();
    let mut compress = Compress::new(mock);

    let request = Request::get("/")
        .header("Accept-Encoding", "gzip")
        .body(())
        .unwrap();

    let response_future = compress.call(request);

    let (_request, send_response) = handle.next_request()
        .unwrap()
        .into_parts();

    send_response.respond(Response::builder()
        .status(200)
        .body(b"hello gzipped world!")
        .expect("send response"));

    let response = response_future.wait()
        .expect("response future");

    assert!(response.headers()
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .any(|v| v == "gzip")
    );

    let body_reader = Cursor::new(response.into_body());
    let mut decoder = read::GzDecoder::new(body_reader);
    let mut decompressed_body = String::new();
    decoder.read_to_string(&mut decompressed_body)
        .expect("decompress");

    assert_eq!("hello gzipped world!", &decompressed_body)
}
