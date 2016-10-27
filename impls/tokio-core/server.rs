extern crate futures;
extern crate tokio_core;

use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::io;
use std::iter;
use std::net::SocketAddr;
use std::rc::Rc;

use futures::Future;
use futures::stream::{self, Stream};
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::io::{read_exact, write_all, Io};
use tokio_core::reactor::{Core, Handle};
use tokio_core::channel::{channel, Sender};

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
          map: Rc<RefCell<HashMap<SocketAddr, Sender<Vec<u8>>>>>)
          -> Box<Future<Item=(), Error=()>> {
    let (reader, writer) = socket.split();
    let (tx, rx) = channel(handle).unwrap();
    let infinite = stream::iter(iter::repeat(()).map(Ok::<(), io::Error>));

    assert!(map.borrow_mut().insert(addr, tx).is_none());

    let buf = Vec::new();
    let map2 = map.clone();
    let reader = infinite.fold((reader, buf), move |(reader, mut buf), ()| {
        let size = read_exact(reader, [0u8]);

        let msg = size.and_then(|(rd, size)| {
            buf.truncate(0);
            buf.extend(iter::repeat(0).take(size[0] as usize));
            read_exact(rd, buf)
        });

        let map = map2.clone();
        msg.map(move |(rd, buf)| {
            for key in map.borrow().values() {
                key.send(buf.clone()).unwrap();
            }
            (rd, buf)
        })
    });

    let writer = rx.fold(writer, |writer, msg| {
        write_all(writer, [msg.len() as u8]).and_then(|(w, _)| {
            write_all(w, msg)
        }).map(|p| p.0)
    });

    let reader = reader.map(|_| ());
    let writer = writer.map(|_| ());
    Box::new(reader.select(writer).then(move |_| {
        assert!(map.borrow_mut().remove(&addr).is_some());
        Ok(())
    }))
}
