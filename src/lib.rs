extern crate nix;
extern crate tokio;

use nix::fcntl::{splice, SpliceFFlags};
use nix::unistd::pipe;
use std::future::Future;
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::pin::Pin;
use std::task::{Context, Poll};

use nix::errno::Errno;
use nix::fcntl::{fcntl, FcntlArg, OFlag};

fn pipe_nonblocking() -> nix::Result<(RawFd, RawFd)> {
    let (read_fd, write_fd) = pipe()?;
    fcntl(read_fd, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))?;
    fcntl(write_fd, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))?;
    Ok((read_fd, write_fd))
}

#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Copy<'a, R: ?Sized, W: ?Sized> {
    reader: &'a mut R,
    read_done: bool,
    writer: &'a mut W,
    pos: usize,
    cap: usize,
    amt: u64,
    read_fd: Option<RawFd>,
    write_fd: Option<RawFd>,
}

pub fn copy<'a, R, W>(reader: &'a mut R, writer: &'a mut W) -> Copy<'a, R, W>
where
    R: AsRawFd + Unpin + ?Sized,
    W: AsRawFd + Unpin + ?Sized,
{
    Copy {
        reader,
        read_done: false,
        writer,
        amt: 0,
        pos: 0,
        cap: 0,
        read_fd: None,
        write_fd: None,
    }
}

const MAX_BUF_SIZE: usize = 4 << 20;

impl<R, W> Future for Copy<'_, R, W>
where
    R: AsRawFd + Unpin + ?Sized,
    W: AsRawFd + Unpin + ?Sized,
{
    type Output = io::Result<u64>;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        //eprintln!("Connecting...");
        // Ensure the pipe has been configured and created.
        {
            let me = &mut *self;
            match (me.read_fd, me.write_fd) {
                (None, None) => match pipe_nonblocking() {
                    Ok((read_fd, write_fd)) => {
                        me.read_fd = Some(read_fd);
                        me.write_fd = Some(write_fd);
                    }
                    Err(err) => return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, err))),
                },
                _ => {}
            };
        }

        //eprintln!("Connected!");

        loop {
            if self.pos == self.cap && !self.read_done {
                //eprintln!("Reading maybe?");

                let me = &mut *self;
                let res = splice(
                    me.reader.as_raw_fd(),
                    None,
                    me.write_fd.unwrap(),
                    None,
                    MAX_BUF_SIZE,
                    SpliceFFlags::SPLICE_F_MOVE
                        // | SpliceFFlags::SPLICE_F_MORE // not added by haproxy
                        | SpliceFFlags::SPLICE_F_NONBLOCK,
                );

                match res {
                    Ok(n) => {
                        //eprintln!("Read some bytes?: {}", n);
                        if n == 0 {
                            self.read_done = true;
                        }
                        self.pos = 0;
                        self.cap = n;
                    }
                    Err(nix::Error::Sys(Errno::EAGAIN)) => return Poll::Pending,
                    Err(nix::Error::Sys(Errno::EINTR)) => continue,
                    Err(err) => {
                        return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, err)));
                    }
                };
            }

            while self.pos < self.cap {
                let remaining = self.cap - self.pos;
                let max = if MAX_BUF_SIZE > remaining {
                    remaining
                } else {
                    MAX_BUF_SIZE
                };

                //eprintln!("Writing bytes...");
                let me = &mut *self;
                let res = splice(
                    me.read_fd.unwrap(),
                    None,
                    me.writer.as_raw_fd(),
                    None,
                    max,
                    SpliceFFlags::SPLICE_F_MOVE
                        // | SpliceFFlags::SPLICE_F_MORE // not added by haproxy
                        | SpliceFFlags::SPLICE_F_NONBLOCK,
                );

                match res {
                    Ok(i) => {
                        //eprintln!("Wrote some bytes?: {}", i);
                        if i == 0 {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::WriteZero,
                                "write zero byte into writer",
                            )));
                        } else {
                            self.pos += i;
                            self.amt += i as u64;
                        }
                    }
                    Err(nix::Error::Sys(Errno::EAGAIN)) => return Poll::Pending,
                    Err(nix::Error::Sys(Errno::EINTR)) => continue,
                    Err(err) => {
                        return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, err)));
                    }
                };
            }

            if self.pos == self.cap && self.read_done {
                // TODO: Should we add this here?
                // let me = &mut *self;
                // ready!(Pin::new(&mut *me.writer).poll_flush(cx))?;
                return Poll::Ready(Ok(self.amt));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::copy;
    use std::fs;
    use std::time::Instant;
    use tokio::fs as tokio_fs;

    #[tokio::test]
    async fn it_can_make_an_end_to_end_copy() {
        let now = Instant::now();

        //fs::write("/tmp/tokio-splice-fixture_end_to_end_copy_file", "hello world").unwrap();
        let mut input = fs::File::open("/tmp/tokio-splice-fixture_end_to_end_copy_file")
            .expect("could not open input file");
        let mut output = fs::File::create("/tmp/tokio-splice-fixture_end_to_end_copy_file-out")
            .expect("could not open output file");

        // eprintln!("...");
        copy(&mut input, &mut output).await.unwrap();

        println!("{}", now.elapsed().as_millis());
    }

    #[tokio::test]
    async fn it_can_make_an_end_to_end_copy_old_school() {
        use tokio::io::AsyncReadExt;

        let now = Instant::now();

        //fs::write("/tmp/tokio-splice-fixture_end_to_end_copy_file", "hello world").unwrap();
        let mut input = tokio_fs::File::open("/tmp/tokio-splice-fixture_end_to_end_copy_file")
            .await
            .expect("could not open input file");
        let mut output =
            tokio_fs::File::create("/tmp/tokio-splice-fixture_end_to_end_copy_file-out")
                .await
                .expect("could not open output file");

        input.copy(&mut output).await.unwrap();

        println!("{}", now.elapsed().as_millis());
    }

    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
