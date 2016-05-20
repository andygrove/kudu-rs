use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::io::{self, ErrorKind, Write};
use std::thread::{self, JoinHandle};
use std::error;
use std::fmt;
use std::time::Instant;

use kudu_pb::rpc_header;

use byteorder::{BigEndian, ByteOrder, LittleEndian, WriteBytesExt};
use eventual::{Future, Complete};
use mio::{
    EventLoop,
    EventSet,
    Handler,
    PollOpt,
    Sender,
    Token,
};
use mio::tcp::TcpStream;
use protobuf::{parse_length_delimited_from, CodedInputStream, Message, ProtobufError};
use protobuf::rt::ProtobufVarint;
use slab::Slab;
use netbuf::Buf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RpcErrorCode {
    // Non-fatal RPC errors. Connection should be left open for future RPC calls.

    /// The application generated an error status. See the message field for
    /// more details.
    ApplicationError,
    /// The specified method was not valid.
    NoSuchMethod,
    /// The specified service was not valid.
    NoSuchService,
    /// The server is overloaded - the client should try again shortly.
    ServerTooBusy,
    /// The request parameter was not parseable, was missing required fields,
    /// or the server does not support the required feature flags.
    InvalidRequest,

    // Fatal errors indicate that the client should shut down the connection.

    FatalUnknown,
    /// The RPC server is already shutting down.
    FatalServerShuttingDown,
    /// Fields of RpcHeader are invalid.
    FatalInvalidRpcHeader,
    /// Could not deserialize RPC request.
    FatalDeserializingRequest,
    /// IPC Layer version mismatch.
    FatalVersionMismatch,
    /// Auth failed.
    FatalUnauthorized,
}

/// An internal error type returned by RPC operations.
#[derive(Debug)]
pub enum Error {
    /// An IO error.
    Io(io::Error),
    /// A Protobuf error.
    Pb(String),
    /// The RPC completed, but the server was not able to service the request.
    Rpc {
        code: RpcErrorCode,
        message: String,
        unsupported_feature_flags: Vec<u32>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl error::Error for Error {
    fn description(&self) -> &str {
        match *self {
            Error::Io(ref error) => error.description(),
            Error::Pb(ref msg) => msg,
            Error::Rpc { ref message, .. } => message,
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match *self {
            Error::Io(ref error) => error.cause(),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}

impl From<ProtobufError> for Error {
    fn from(error: ProtobufError) -> Error {
        match error {
            ProtobufError::IoError(error) => Error::Io(error),
            ProtobufError::WireError(msg) => Error::Pb(msg),
            ProtobufError::MessageNotInitialized { message } =>
                // This should never happen, all Protobuf messages are initialized internally.
                panic!("Protobuf message not initialized: {}", message),
        }
    }
}

type Loop = EventLoop<ConnectionManager>;

pub struct Messenger {
    channel: Sender<Command>,
    thread: JoinHandle<io::Result<()>>,
}

impl Messenger {
    pub fn new() -> io::Result<Messenger> {
        let mut event_loop = try!(EventLoop::new());
        let channel = event_loop.channel();
        let thread = thread::spawn(move || {
            let mut connection_manager = ConnectionManager::new();
            event_loop.run(&mut connection_manager)
        });
        Ok(Messenger {
            channel: channel,
            thread: thread,
        })
    }

    pub fn send(&self,
                addr: SocketAddr,
                service_name: &'static str,
                method_name: &'static str,
                timeout: Instant,
                request: Box<Message>,
                response: Box<Message>) -> Future<Response, Error> {
        let (complete, future) = Future::pair();
        let request = Command::Request {
            addr: addr,
            service_name: service_name,
            method_name: method_name,
            timeout: timeout,
            request_message: request,
            response_message: response,
            complete: complete,
        };

        self.channel.send(request).unwrap();
        future
    }
}

#[derive(Debug)]
struct ConnectionManager {
    slab: Slab<Connection, Token>,
    index: HashMap<SocketAddr, Token>
}

impl ConnectionManager {
    fn new() -> ConnectionManager {
        ConnectionManager {
            slab: Slab::new(512),
            index: HashMap::new(),
        }
    }
}

impl Handler for ConnectionManager {

    type Timeout = ();
    type Message = Command;

    fn ready(&mut self, event_loop: &mut Loop, token: Token, events: EventSet) {
        if events.is_hup() {
            let connection = self.slab.remove(token);
            debug!("hup; connection: {:?}, events: {:?}", connection, events);
        } else if events.is_error() {
            let connection = self.slab.remove(token);
            warn!("error; connection: {:?}, events: {:?}", connection, events);
        } else {
            let connection = &mut self.slab[token];
            trace!("ready; connection: {:?}, events: {:?}", connection, events);
        }
    }

    fn notify(&mut self, event_loop: &mut Loop, command: Command) {
        match command {
            Command::Shutdown => {
                event_loop.shutdown();
            },
            Command::Request { .. } => {
            },
        }
    }

    fn timeout(&mut self, event_loop: &mut Loop, timeout: Self::Timeout) {
    }

    fn interrupted(&mut self, event_loop: &mut Loop) {
    }

    fn tick(&mut self, event_loop: &mut Loop) {
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    Initiating,
    Connected
}

#[derive(Debug)]
struct Connection {
    state: ConnectionState,
    stream: TcpStream,
    addr: SocketAddr,
    recv_buf: Buf,
    send_buf: Buf,
}

impl Connection {

    fn connect(event_loop: &mut Loop, token: Token, addr: SocketAddr) -> io::Result<Connection> {
        debug!("connect; addr: {:?}", addr);
        let mut stream = try!(TcpStream::connect(&addr));
        let mut send_buf = Buf::new();

        // Add the connection header to the send buffer
        send_buf.write(b"hrpc\x09\0\0");

        // Add the SASL negotiation message to the send buffer
        let mut sasl_header = rpc_header::RequestHeader::new();
        sasl_header.set_call_id(-33);
        let mut sasl_negotiate = rpc_header::SaslMessagePB::new();
        sasl_negotiate.set_state(rpc_header::SaslMessagePB_SaslState::NEGOTIATE);

        let sasl_header_len = sasl_header.compute_size();
        let sasl_negotiate_len = sasl_negotiate.compute_size();
        let len = sasl_header_len + sasl_header_len.len_varint() +
                  sasl_negotiate_len + sasl_negotiate_len.len_varint();

        // TODO: remove the expects once there is an internal error type
        try!(send_buf.write_u32::<BigEndian>(len));
        sasl_header.write_to_with_cached_sizes(&mut send_buf)
                   .expect("unable to serialize sasl header");
        sasl_negotiate.write_to_with_cached_sizes(&mut send_buf)
                      .expect("unable to serialize sasl negotiate");

        // Optimistically flush the connection header and SASL negotiation to the TCP socket. Even
        // though the socket hasn't yet been registered, and the connection is probably not yet
        // complete, this will usually succeed because the socket will have sufficient internal
        // buffer space.
        //
        // If all bytes are flushed, then register the socket for readable events in order to
        // listen for the SASL NEGOTIATE response. Otherwise, register for the writable event so
        // sending can continue later.
        while !send_buf.is_empty() {
            match send_buf.write_to(&mut stream) {
                Err(ref error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
                _ => continue,
            }
        }
        if send_buf.is_empty() {
            try!(event_loop.register(&mut stream, token,
                                     EventSet::hup() | EventSet::error() | EventSet::readable(),
                                     PollOpt::edge() | PollOpt::oneshot()));
        } else {
            try!(event_loop.register(&mut stream, token,
                                     EventSet::hup() | EventSet::error() | EventSet::writable(),
                                     PollOpt::edge() | PollOpt::oneshot()));
        }

        Ok(Connection {
            state: ConnectionState::Initiating,
            stream: stream,
            addr: addr,
            recv_buf: Buf::new(),
            send_buf: send_buf,
        })
    }

    fn ready(&mut self, events: EventSet) -> io::Result<()> {
        match self.state {
            ConnectionState::Initiating => {
                if events.is_readable() {
                    assert!(!events.is_writable());
                    assert!(self.send_buf.is_empty());
                }
            },
            ConnectionState::Connected => {
            },
        };
        Ok(())
    }

    fn readable(&mut self) -> io::Result<()> {
        loop {
            // Read, or continue reading, a message from the socket into the receive buffer.
            if self.recv_buf.len() < 4 {
                let needed = 4 - self.recv_buf.len();
                if try!(self.recv(needed)) < needed {
                    warn!("incomplete message length read");
                    return Ok(());
                }
            }

            let msg_len = LittleEndian::read_u32(&self.recv_buf[..4]) as usize;
            let msg_bytes = self.recv_buf.len() - 4;
            if self.recv_buf.len() - 4 < msg_len {
                let needed = msg_len + 4 - self.recv_buf.len();
                if try!(self.recv(needed)) < needed {
                    // As opposed to the message length, we expect the message body to be split
                    // across multiple packets.
                    debug!("incomplete message read");
                    return Ok(());
                }
            }

            // The whole message has been read
            self.recv_buf.consume(4);

            let (header, size) = {
                let mut coded_stream = CodedInputStream::from_bytes(&self.recv_buf[..]);
                let header = parse_length_delimited_from::<rpc_header::ResponseHeader>(&mut coded_stream);
                (header, coded_stream.pos() as usize)
            };
            self.recv_buf.consume(size);

        }

        match self.state {
            ConnectionState::Initiating => {
                // Read the 
            },
            ConnectionState::Connected => {
            },
        }
        Ok(())
    }

    /// Flushes the send buffer to the socket.
    fn flush(&mut self) -> io::Result<usize> {
        let Connection { ref mut stream, ref mut send_buf, .. } = *self;
        let mut sent = 0;
        while !send_buf.is_empty() {
            match send_buf.write_to(stream) {
                Ok(amount) => sent += amount,
                Err(ref error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(sent)
    }

    /// Attempts to read at least `min` bytes from the socket into the receive buffer.
    /// Fewer bytes may be read if there is no data available.
    fn recv(&mut self, min: usize) -> io::Result<usize> {
        let Connection { ref mut stream, ref mut recv_buf, .. } = *self;
        let mut received = 0;
        while received < min {
            match recv_buf.read_from(stream) {
                Ok(amount) => received += amount,
                Err(ref error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(received)
    }

    fn event_set(&self) -> EventSet {
        EventSet::readable() | EventSet::writable() | EventSet::hup() | EventSet::error()
    }
}

#[derive(Debug)]
enum Command {
    Shutdown,
    Request {
        addr: SocketAddr,
        service_name: &'static str,
        method_name: &'static str,
        timeout: Instant,
        request_message: Box<Message>,
        response_message: Box<Message>,
        complete: Complete<Response, Error>,
    },
}

struct Response {
    request_message: Box<Message>,
    response_message: Box<Message>,
    sidecars: Vec<Vec<u8>>,
}