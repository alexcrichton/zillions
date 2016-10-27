#[macro_use]
extern crate futures;
extern crate tokio_core;

use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::io;
use std::iter;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;

use futures::{Future, Async, Poll};
use futures::stream::{self, Stream, Fuse};
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::io::{read_exact, write_all};
use tokio_core::reactor::{Core, Handle};
use tokio_core::channel::{channel, Sender, Receiver};

const MAX_BUFFERED: usize = 256;

fn main() {
    let addr = env::args().skip(1).next();
    let addr = addr.unwrap_or("127.0.0.1:8080".to_string());
    let addr: SocketAddr = addr.parse().unwrap();

    let mut core = Core::new().unwrap();
    let handle = core.handle();

    let srv = TcpListener::bind(&addr, &handle).unwrap();
    println!("listening on {}", addr);
    let map = Rc::new(RefCell::new(HashMap::new()));

    let srv = srv.incoming().for_each(|(socket, addr)| {
        let handle2 = handle.clone();
        let map = map.clone();
        handle.spawn(futures::lazy(move || {
            client(socket, &handle2, addr, map)
        }));
        Ok(())
    });

    core.run(srv).unwrap();
}

fn client(socket: TcpStream,
          handle: &Handle,
          addr: SocketAddr,
          map: Rc<RefCell<HashMap<SocketAddr, Sender<MyBuf>>>>)
          -> Box<Future<Item=(), Error=()>> {
    let socket = Rc::new(socket);
    let (tx, rx) = channel(handle).unwrap();
    let infinite = stream::iter(iter::repeat(()).map(Ok::<(), io::Error>));

    assert!(map.borrow_mut().insert(addr, tx).is_none());

    let map2 = map.clone();
    let reader = infinite.fold(MySocket(socket.clone()), move |reader, ()| {
        let size = read_exact(reader, [0u8]);

        let msg = size.and_then(|(rd, size)| {
            let buf = vec![0u8; size[0] as usize];
            read_exact(rd, buf)
        });

        let map = map2.clone();
        msg.map(move |(rd, buf)| {
            let buf = MyBuf(Arc::new(buf));
            for key in map.borrow().values() {
                key.send(buf.clone()).unwrap();
            }
            rd
        })
    });

    let writer = ClientWrite {
        rx: rx.fuse(),
        messages: Vec::new(),
        current: None,
        writer: MySocket(socket),
    };

    let reader = reader.map(|_| ()).map_err(|_| ());
    let writer = writer.map(|_| ()).map_err(|_| ());
    Box::new(reader.select(writer).then(move |_| {
        assert!(map.borrow_mut().remove(&addr).is_some());
        Ok(())
    }))
}

#[derive(Clone)]
struct MyBuf(Arc<Vec<u8>>);

impl AsRef<[u8]> for MyBuf {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

struct ClientWrite {
    rx: Fuse<Receiver<MyBuf>>,
    messages: Vec<MyBuf>,
    current: Option<Box<Future<Item=(), Error=()>>>,
    writer: MySocket,
}

impl Future for ClientWrite {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<(), ()> {
        loop {
            while self.current.is_some() || self.messages.len() > 0 {
                let mut done = false;
                if let Some(ref mut cur) = self.current {
                    done = try!(cur.poll()).is_ready();
                }
                if done {
                    self.current = None;
                }
                if self.messages.len() == 0 {
                    break
                }
                let msg = self.messages.remove(0);
                let header = write_all(self.writer.clone(), [msg.0.len() as u8]);
                let body = header.and_then(|(wr, _)| {
                    write_all(wr, msg)
                });
                let body = body.map(|_| ()).map_err(|_| ());
                self.current = Some(Box::new(body));
            }

            match try_ready!(self.rx.poll().map_err(|_| ())) {
                None => {
                    if self.messages.len() == 0 && self.current.is_none() {
                        return Ok(().into())
                    } else {
                        return Ok(Async::NotReady)
                    }
                }
                Some(msg) => {
                    if self.messages.len() < MAX_BUFFERED {
                        self.messages.push(msg);
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct MySocket(Rc<TcpStream>);

impl io::Read for MySocket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self.0).read(buf)
    }
}

impl io::Write for MySocket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self.0).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}
