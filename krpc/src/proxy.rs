use std::fmt;
use std::net::SocketAddr;

use futures::{
    Async,
    AsyncSink,
    Future,
    Poll,
    Sink,
    Stream,
};
use futures::sync::{mpsc, oneshot};
use tacho;
use tokio::reactor::{
    Handle,
    Remote,
};

use Options;
use RawResponse;
use RawResponseFuture;
use Request;
use Rpc;
use connection::{Connection, ConnectionNew};
use transport::{Transport, TransportNew};
use negotiator::Negotiator;

#[derive(Clone, Debug)]
pub struct Proxy {
    sender: mpsc::Sender<Rpc>,
}

/// The result of an asynchronous remote method call.
#[must_use]
#[derive(Debug)]
pub enum AsyncSend {
    /// The RPC was sent. The response will be returned through the included future.
    Ready(oneshot::Receiver<RawResponse>),
    /// The connection is not ready.
    ///
    /// The current task will be scheduled to receive a notification when the `Proxy` is ready to
    /// send.
    NotReady(Request),
}

impl Proxy {

    pub fn spawn(addr: SocketAddr, options: Options, remote: &Remote) -> Proxy {
        trace!("spawn!");
        let (sender, receiver) = mpsc::channel(options.max_rpcs_in_flight as usize);
        let metrics = options.scope.as_ref().map(|scope| Metrics::new(&addr, scope.clone()));
        remote.spawn(move |handle| ProxyTask {
            addr: addr,
            options: options,
            handle: handle.clone(),
            receiver,
            connection_state: ConnectionState::Quiesced,
            metrics,
        });
        Proxy { sender }
    }

    /// Polls the proxy to determine if there is guaranteed to be capacity to send at least one
    /// RPC without waiting.
    ///
    /// Returns `Async::Ready(_)` if there is sufficient capacity, or returns `Async::NotReady` if
    /// the proxy is not guaranteed to have capacity.
    ///
    /// This method may only be called from inside the context of a task or future.
    pub fn poll_ready(&mut self) -> Async<()> {
        match self.sender.poll_ready() {
            Ok(async) => async,
            Err(_) => unreachable!(),
        }
    }

    /// Call a remote method asynchronously.
    ///
    /// Typically users will not call this directly, but rather through a generated service trait
    /// implemented by `Proxy`.
    pub fn send(&mut self, request: Request) -> RawResponseFuture {
        let (completer, receiver) = oneshot::channel();
        let rpc = Rpc {
            request,
            completer
        };

        match self.sender.start_send(rpc) {
            Ok(AsyncSink::Ready) => (),
            Ok(AsyncSink::NotReady(_)) => panic!("Proxy not ready"),
            Err(..) => unreachable!(),
        }

        receiver
    }
}

enum ConnectionState {
    Quiesced,
    // TODO:
    // Resolving,
    Connecting(TransportNew),
    Negotiating(Negotiator),
    Connected(Connection),
}

impl fmt::Debug for ConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ConnectionState::Quiesced => write!(f, "Quiesced"),
            ConnectionState::Connecting(_) => write!(f, "Connecting"),
            ConnectionState::Negotiating(_) => write!(f, "Negotiating"),
            ConnectionState::Connected(ref connection) => connection.fmt(f),
        }
    }
}

struct ProxyTask {
    addr: SocketAddr,
    options: Options,
    handle: Handle,
    receiver: mpsc::Receiver<Rpc>,
    connection_state: ConnectionState,
    metrics: Option<Metrics>,
}

impl Future for ProxyTask {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        trace!("{:?}: poll", self);
        let ProxyTask { addr,
                        ref options,
                        ref handle,
                        ref mut receiver,
                        ref mut connection_state,
                        ref mut metrics } = *self;
        use self::ConnectionState::*;
        // NLL hack.
        loop {
            let state = match *connection_state {
                Quiesced => {
                    // Assume wakeup due to an RPC being ready to send.
                    Connecting(Transport::connect(addr, options.clone(), handle))
                },
                Connecting(ref mut new) => {
                    match new.poll() {
                        Ok(Async::Ready(transport)) => {
                            Negotiating(Negotiator::negotiate(transport))
                        },
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Err(error) => {
                            error!("connect error: {}", error);
                            if let Some(ref mut metrics) = *metrics {
                                metrics.connecting_errors.incr(1);
                            }
                            // TODO: log and reconnect
                            unimplemented!()
                        }
                    }
                },
                Negotiating(ref mut negotiator) => {
                    match negotiator.poll() {
                        Ok(Async::Ready(transport)) => {
                            Connected(Connection::new(transport))
                        },
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Err(error) => {
                            error!("negotiation error: {}", error);
                            if let Some(ref mut metrics) = *metrics {
                                metrics.connecting_errors.incr(1);
                            }
                            // TODO: log and reconnect
                            unimplemented!()

                        },
                    }
                },
                Connected(ref mut conn) => {
                    // Send all queued messages.
                    loop {
                        match conn.poll_ready() {
                            Ok(Async::Ready(_)) => {
                                match receiver.poll() {
                                    Ok(Async::Ready(Some(request))) => conn.send(request).expect("not handled"),
                                    Ok(Async::Ready(None)) => {
                                        // TODO: all senders dropped
                                        unimplemented!()
                                    }
                                    Ok(Async::NotReady) => break,
                                    Err(()) => unreachable!(),
                                }
                            },
                            Ok(Async::NotReady) => (),
                            Err(error) => {
                                error!("poll error: {}", error);
                                // TODO: log and reconnect
                                unimplemented!()
                            }
                        }
                    }

                    if let Err(error) = conn.poll() {
                        error!("poll error: {}", error);
                        // TODO: log and reconnect
                        unimplemented!()
                    }
                    return Ok(Async::NotReady);
                },
            };
            *connection_state = state;
        }
    }
}

impl fmt::Debug for ProxyTask {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut debug = f.debug_struct("ProxyTask");
        debug.field("addr", &format_args!("{}", &self.addr));
        debug.field("core", &self.handle.id());
        match self.connection_state {
            ConnectionState::Quiesced => debug.field("state", &self.connection_state),
            ConnectionState::Connecting(_) => debug.field("state", &self.connection_state),
            ConnectionState::Negotiating(_) => debug.field("state", &self.connection_state),
            ConnectionState::Connected(ref connection) => debug.field("connection", connection),
        };
        debug.finish()
    }
}

struct Metrics {
    /// Number of failures while attempting to connect.
    connecting_errors: tacho::Counter,

    /// Number of failures while negotiating.
    negotiating_errors: tacho::Counter,

    /// Number of failures while connected.
    connected_errors: tacho::Counter,
}

impl Metrics {
    fn new(addr: &SocketAddr, scope: tacho::Scope) -> Metrics {
        let errors = scope.prefixed("krpc")
                          .labeled("addr", addr);

        let connecting_errors = errors.clone().labeled("state", "connecting").counter("proxy_errors");
        let negotiating_errors = errors.clone().labeled("state", "negotiating").counter("proxy_errors");
        let connected_errors = errors.labeled("state", "connected").counter("proxy_errors");
        Metrics {
            connecting_errors,
            negotiating_errors,
            connected_errors,
        }
    }
}
