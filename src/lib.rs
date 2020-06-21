//! Asynchronous pipe(2) library using tokio.
//!
//! # Example
//!
//! ```
//! use tokio::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let (mut r, mut w) = tokio_pipe::pipe()?;
//!
//!     w.write_all(b"HELLO, WORLD!").await?;
//!
//!     let mut buf = bytes::BytesMut::with_capacity(100);
//!     r.read_buf(&mut buf).await?;
//!
//!     assert_eq!(&buf, &b"HELLO, WORLD!"[..]);
//!     Ok(())
//! }
//! ```
use std::cmp;
use std::ffi::c_void;
use std::io;
use std::mem::{self, MaybeUninit};
use std::os::unix::io::{AsRawFd, IntoRawFd, RawFd};
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Buf, BufMut};
use mio::event::Evented;
use mio::unix::EventedFd;
use mio::{Poll as MioPoll, PollOpt, Ready, Token};
use tokio::io::{AsyncRead, AsyncWrite, PollEvented};

#[cfg(target_os = "macos")]
const MAX_LEN: usize = <libc::c_int>::MAX as usize - 1;

#[cfg(not(target_os = "macos"))]
const MAX_LEN: usize = <libc::ssize_t>::MAX as usize;

struct PipeFd(RawFd);

impl Evented for PipeFd {
    fn register(
        &self,
        poll: &MioPoll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.0).register(poll, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &MioPoll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        EventedFd(&self.0).reregister(poll, token, interest, opts)
    }

    fn deregister(&self, poll: &MioPoll) -> io::Result<()> {
        EventedFd(&self.0).deregister(poll)
    }
}

impl io::Read for PipeFd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let ret = unsafe {
            libc::read(
                self.0,
                buf.as_mut_ptr() as *mut c_void,
                cmp::min(buf.len(), MAX_LEN),
            )
        };
        if ret == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(ret as usize)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        let ret = unsafe {
            libc::readv(
                self.0,
                bufs.as_ptr() as *const libc::iovec,
                cmp::min(bufs.len(), libc::c_int::MAX as usize) as libc::c_int,
            )
        };
        if ret == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(ret as usize)
    }
}

impl io::Write for PipeFd {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let ret = unsafe {
            libc::write(
                self.0,
                buf.as_ptr() as *mut c_void,
                cmp::min(buf.len(), MAX_LEN),
            )
        };
        if ret == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(ret as usize)
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let ret = unsafe {
            libc::writev(
                self.0,
                bufs.as_ptr() as *const libc::iovec,
                cmp::min(bufs.len(), libc::c_int::MAX as usize) as libc::c_int,
            )
        };
        if ret == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(ret as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl PipeFd {
    fn close(&mut self) -> io::Result<()> {
        let ret = unsafe { libc::close(self.0) };
        if ret == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for PipeFd {
    fn drop(&mut self) {
        self.close().ok();
    }
}

/// Pipe read
pub struct PipeRead(PollEvented<PipeFd>);

impl AsyncRead for PipeRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }

    fn poll_read_buf<B: BufMut>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut B,
    ) -> Poll<io::Result<usize>>
    where
        Self: Sized,
    {
        Pin::new(&mut self.0).poll_read_buf(cx, buf)
    }

    unsafe fn prepare_uninitialized_buffer(&self, buf: &mut [MaybeUninit<u8>]) -> bool {
        self.0.prepare_uninitialized_buffer(buf)
    }
}

impl AsRawFd for PipeRead {
    fn as_raw_fd(&self) -> RawFd {
        self.0.get_ref().0
    }
}

impl IntoRawFd for PipeRead {
    fn into_raw_fd(self) -> RawFd {
        let fd = self.0.get_ref().0;
        mem::forget(self);
        fd
    }
}

/// Pipe write
pub struct PipeWrite(PollEvented<PipeFd>);

impl AsRawFd for PipeWrite {
    fn as_raw_fd(&self) -> RawFd {
        self.0.get_ref().0
    }
}

impl IntoRawFd for PipeWrite {
    fn into_raw_fd(self) -> RawFd {
        let fd = self.0.get_ref().0;
        mem::forget(self);
        fd
    }
}

impl AsyncWrite for PipeWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Poll::Ready(self.0.get_mut().close())
    }

    fn poll_write_buf<B: Buf>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut B,
    ) -> Poll<Result<usize, io::Error>>
    where
        Self: Sized,
    {
        Pin::new(&mut self.0).poll_write_buf(cx, buf)
    }
}

fn sys_pipe() -> io::Result<(RawFd, RawFd)> {
    let mut pipefd = [0; 2];
    let ret = unsafe { libc::pipe2(pipefd.as_mut_ptr(), libc::O_NONBLOCK) };
    if ret == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok((pipefd[0], pipefd[1]))
}

/// Open pipe
pub fn pipe() -> io::Result<(PipeRead, PipeWrite)> {
    let (r, w) = sys_pipe()?;
    Ok((
        PipeRead(PollEvented::new(PipeFd(r))?),
        PipeWrite(PollEvented::new(PipeFd(w))?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::prelude::*;

    #[tokio::test]
    async fn test() {
        let (mut r, mut w) = pipe().unwrap();

        let w_task = tokio::spawn(async move {
            for n in 0..65535 {
                w.write_u32(n).await.unwrap();
            }
            //w.shutdown().await.unwrap();
        });

        let r_task = tokio::spawn(async move {
            let mut n = 0u32;
            let mut buf = bytes::BytesMut::with_capacity(4 * 100);
            while r.read_buf(&mut buf).await.unwrap() != 0 {
                for x in buf.chunks(4) {
                    assert_eq!(x, n.to_be_bytes());
                    n += 1;
                }
                buf.clear()
            }
        });
        tokio::try_join!(w_task, r_task).unwrap();
    }
}