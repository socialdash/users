use std::fmt;

use tokio_core::reactor::{Handle};
use hyper;
use futures::future::IntoFuture;
use futures::{future, Future};
use futures::sync::{mpsc, oneshot};
use futures::stream::{Stream};
use futures::sink::Sink;
use serde_json;
use serde::de::Deserialize;
use juniper::FieldError;

use super::utils;
use ::config::Config;

pub type ClientResult = Result<String, Error>;

pub struct Client {
    client: hyper::Client<hyper::client::HttpConnector>,
    tx: mpsc::Sender<Payload>,
    rx: mpsc::Receiver<Payload>,
    max_retries: usize,
}

impl Client {
    pub fn new(config: &Config, handle: &Handle) -> Self {
        let max_retries = config.gateway.http_client_retries;
        let (tx, rx) = mpsc::channel::<Payload>(config.gateway.http_client_buffer_size);
        let client = hyper::Client::new(handle);
        Client { client, tx, rx, max_retries }
    }

    pub fn stream(self) -> Box<Stream<Item=(), Error=()>> {
        let Self { client, tx: _, rx, max_retries: _ } = self;
        Box::new(
            rx.and_then(move |payload| {
                Self::send_request(&client, payload).map(|_| ()).map_err(|_| ())
            })
        )
    }

    pub fn handle(&self) -> ClientHandle {
        ClientHandle {
            tx: self.tx.clone(),
            max_retries: self.max_retries,
        }
    }

    fn send_request(client: &hyper::Client<hyper::client::HttpConnector>, payload: Payload) -> Box<Future<Item=(), Error=()>> {
        let Payload { url, method, body: maybe_body, callback } = payload;

        let uri = match url.parse() {
        Ok(val) => val,
        Err(err) => {
            error!("Url `{}` passed to http client cannot be parsed: `{}`", url, err);
            return Box::new(callback.send(Err(Error::Parse(format!("Cannot parse url `{}`", url)))).into_future().map(|_| ()).map_err(|_| ()))
        }
        };
        let mut req = hyper::Request::new(method, uri);
        for body in maybe_body.iter() {
        req.set_body(body.clone());
        }

        let task = client.request(req)
        .map_err(|err| Error::Network(err))
        .and_then(move |res| {
            let status = res.status();
            let body_future: Box<future::Future<Item = String, Error = Error>> =
            Box::new(utils::read_body(res.body()).map_err(|err| Error::Network(err)));
            match status {
            hyper::StatusCode::Ok =>
                body_future,

            _ =>
                Box::new(
                body_future.and_then(move |body| {
                    let message = serde_json::from_str::<ErrorMessage>(&body).ok();
                    let error = Error::Api(status, message);
                    future::err(error)
                })
                )
            }
            })
            .then(|result| callback.send(result))
            .map(|_| ()).map_err(|_| ());

        Box::new(task)
    }

}

#[derive(Clone)]
pub struct ClientHandle {
  tx: mpsc::Sender<Payload>,
  max_retries: usize,
}

impl ClientHandle {

    pub fn request<T>(&self, method: hyper::Method, url: String, body: Option<String>) -> Box<Future<Item=T, Error=Error>>
        where T: for <'a> Deserialize<'a> + 'static
    {
        Box::new(
            self.send_request_with_retries(method, url, body, None, self.max_retries)
                .and_then(|response| {
                    serde_json::from_str::<T>(&response)
                        .map_err(|err| Error::Parse(format!("{}", err)))
                })
        )
    }

    fn send_request_with_retries(&self, method: hyper::Method, url: String, body: Option<String>, last_err: Option<Error>, retries: usize) -> Box<Future<Item=String, Error=Error>> {
        if retries == 0 {
            let error = last_err.unwrap_or(Error::Unknown("Unexpected missing error in send_request_with_retries".to_string()));
            Box::new(
                future::err(error)
            )
        } else {
            let self_clone = self.clone();
            let method_clone = method.clone();
            let body_clone = body.clone();
            let url_clone = url.clone();
            Box::new(
                self.send_request(method, url, body)
                    .or_else(move |err| {
                        match err {
                            Error::Network(err) => {
                                warn!("Failed to fetch `{}` with error `{}`, retrying... Retries left {}", url_clone, err, retries);
                                self_clone.send_request_with_retries(method_clone, url_clone, body_clone, Some(Error::Network(err)), retries - 1)
                            }
                            _ => Box::new(future::err(err))
                        }
                    })
            )

        }
    }

    fn send_request(&self, method: hyper::Method, url: String, body: Option<String>) -> Box<Future<Item=String, Error=Error>> {
        info!("Starting outbound http request: {} {} with body {}", method, url, body.clone().unwrap_or_default());
        let url_clone = url.clone();
        let method_clone = method.clone();

        let (tx, rx) = oneshot::channel::<ClientResult>();
        let payload = Payload {
            url,
            method,
            body,
            callback: tx,
        };


        let future = self.tx.clone().send(payload)
        .map_err(|err| {
            Error::Unknown(format!("Unexpected error sending http client request params to channel: {}", err))
        })
        .and_then(|_| {
            rx.map_err(|err| {
                Error::Unknown(format!("Unexpected error receiving http client response from channel: {}", err))
            })
        })
        .and_then(|result| result)
        .map_err(move |err| {
            error!("{} {} : {}", method_clone, url_clone, err);
            err
        });

        Box::new(future)
    }
}

struct Payload {
    pub url: String,
    pub method: hyper::Method,
    pub body: Option<String>,
    pub callback: oneshot::Sender<ClientResult>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorMessage {
    pub code: u16,
    pub message: String
}

#[derive(Debug)]
pub enum Error {
    Api(hyper::StatusCode, Option<ErrorMessage>),
    Network(hyper::Error),
    Parse(String),
    Unknown(String),
}


impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            &Error::Api(ref status, Some(ref error_message)) => {
                write!(f, "Http client 100: Api error: status: {}, code: {}, message: {}", status, error_message.code, error_message.message)
            },
            &Error::Api(status, None) => {
                write!(f, "Http client 100: Api error: status: {}", status)
            },
            &Error::Network(ref err) => {
                write!(f, "Http client 200: Network error: {:?}", err)
            },
            &Error::Parse(ref err) => {
                write!(f, "Http client 300: Parse error: {}", err)
            }
            &Error::Unknown(ref err) => {
                write!(f, "Http client 400: Unknown error: {}", err)
            }
        }
    }
}
