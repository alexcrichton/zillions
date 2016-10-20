extern crate byteorder;
extern crate clap;

#[macro_use]
extern crate futures;
extern crate futures_cpupool;

#[macro_use]
extern crate tokio_core;

use futures::{Async, Poll, Future};
use futures::stream::Stream;


mod all {
    use futures::{Async, Poll, Future};
    enum ElemState<T> where T: Future {
        Pending(T),
        Done(T::Item),
    }

    pub struct All<T> where T: Future {
        elems: Vec<ElemState<T>>,
    }

    impl <T> All<T> where T: Future {
        pub fn new<I>(futures: I) -> All<T> where I: Iterator<Item=T> {
            let mut result = All { elems: Vec::new() };
            for f in futures {
                result.elems.push(ElemState::Pending(f))
            }
            result
        }
    }

    impl <T> Future for All<T> where T: Future {
        type Item = Vec<T::Item>;
        type Error = T::Error;

        fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
            let mut all_done = true;

            for idx in 0 .. self.elems.len() {
                let done_val = match &mut self.elems[idx] {
                    &mut ElemState::Pending(ref mut t) => {
                        match t.poll() {
                            Ok(Async::Ready(t)) => t,
                            Ok(Async::NotReady) => {
                                all_done = false;
                                continue
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    &mut ElemState::Done(ref mut _v) => continue,
                };

                self.elems[idx] = ElemState::Done(done_val);
            }

            if all_done {
                let mut result = Vec::new();
                let elems = ::std::mem::replace(&mut self.elems, Vec::new());
                for e in elems.into_iter() {
                    match e {
                        ElemState::Done(t) => result.push(t),
                        _ => unreachable!(),
                    }
                }
                Ok(Async::Ready(result))
            } else {
                Ok(Async::NotReady)
            }
        }
    }
}

struct Knot<F, S, T, E>
    where F: Fn(S) -> T,
          T: Future<Item=(S, bool), Error=E>
{
    f: F,
    in_progress: T,
}

fn tie_knot<F, S, T, E>(f: F, initial_state: S) -> Knot<F, S, T, E>
    where F: Fn(S) -> T,
          T: Future<Item=(S, bool), Error=E>,
{
    let in_progress = f(initial_state);
    Knot {
        f: f,
        in_progress: in_progress,
    }
}

impl <F, S, T, E> Future for Knot<F, S, T, E>
    where F: Fn(S) -> T,
          T: Future<Item=(S, bool), Error=E>
{
    type Item = S;
    type Error = E;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let (s, more) = try_ready!(self.in_progress.poll());
        if more {
            self.in_progress = (self.f)(s);
            Ok(Async::NotReady)
        } else {
            Ok(Async::Ready(s))
        }
    }
}

struct Reading<R> where R: ::std::io::Read {
    reader: Option<R>,
    buffer: Vec<u8>,
    pos: usize,
    frame_end: Option<u8>,
}

impl <R> Reading<R> where R: ::std::io::Read {
    fn new(reader: R) -> Reading<R> {
        Reading {
            reader: Some(reader),
            buffer: Vec::new(),
            pos: 0,
            frame_end: None,
        }
    }
}

impl <R> Future for Reading<R> where R: ::std::io::Read {
    type Item = (Option<Vec<u8>>, R);
    type Error = ::std::io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            if let Some(frame_end) = self.frame_end {
                let n = try_nb!(self.reader.as_mut().unwrap().read(&mut self.buffer[self.pos..frame_end as usize]));
                self.pos += n;
                if self.pos == frame_end as usize {
                    self.pos = 0;
                    let result = ::std::mem::replace(&mut self.buffer, Vec::new());
                    self.frame_end = None;
                    return Ok(Async::Ready((Some(result), self.reader.take().unwrap())))
                }
            } else {
                let mut buf = [0u8];
                let n = try_nb!(self.reader.as_mut().unwrap().read(&mut buf));
                if n == 0 { // EOF
                    return Ok(Async::Ready((None, self.reader.take().unwrap())))
                }

                self.frame_end = Some(buf[0]);
                self.buffer = vec![0; buf[0] as usize];
            }
        }
    }
}

pub struct Writing<W> where W: ::std::io::Write {
    writer: Option<W>,
    message: Vec<u8>,
    pos: usize,
    wrote_header: bool,
}

impl <W> Writing<W> where W: ::std::io::Write {
    fn new(writer: W, message: Vec<u8>) -> Writing<W> {
        Writing {
            writer: Some(writer),
            message: message,
            pos: 0,
            wrote_header: false,
        }
    }
}

impl <W> Future for Writing<W> where W: ::std::io::Write {
    type Item = W;
    type Error = ::std::io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        unimplemented!()
    }
}


fn new_task(handle: &::tokio_core::reactor::Handle,
            addr: &::std::net::SocketAddr) -> Box<Future<Item=(), Error=::std::io::Error> + Send> {
    let publisher = ::tokio_core::net::TcpStream::connect(addr, handle);
    let mut subscribers = Vec::new();
    for _ in 0..3 {
        subscribers.push(::tokio_core::net::TcpStream::connect(addr, handle));
    }
    Box::new(publisher.join(::all::All::new(subscribers.into_iter())).and_then(|(publisher, subscribers)| {
        println!("connected");
        Ok(())
    }))
}

pub fn run() -> Result<(), ::std::io::Error> {
    use clap::{App, Arg};
    let matches = App::new("Zillions benchmarker")
        .version("0.0.0")
        .about("Does awesome things")
        .arg(Arg::with_name("EXECUTABLE")
             .required(true)
             .index(1)
             .help("The executable to benchmark"))
        .get_matches();

    let executable = matches.value_of("EXECUTABLE").unwrap();

    println!("exectuable: {}", executable);

    let addr_str = "127.0.0.1:8080";
    let addr = match addr_str.parse::<::std::net::SocketAddr>() {
        Ok(a) => a,
        Err(e) => {
            panic!("failed to parse socket address {}", e);
        }
    };

    let _child = ::std::process::Command::new(executable)
        .arg(addr_str)
        .spawn();



    // start tokio reactor
    let mut core = try!(::tokio_core::reactor::Core::new());

    let handle = core.handle();

    let pool = ::futures_cpupool::CpuPool::new_num_cpus();

    let f = pool.spawn(new_task(&handle, &addr));

    try!(core.run(f));

    Ok(())
}

pub fn main() {
    run().expect("top level error");
}
