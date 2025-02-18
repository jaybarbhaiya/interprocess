//! Methods and trait implementations for `PipeStream`.

mod split_owned;
pub(crate) use split_owned::UNWRAP_FAIL_MSG;

use super::{super::set_nonblocking_for_stream, *};
use crate::{
    os::windows::{
        named_pipe::{convert_and_encode_path, PipeMode},
        weaken_buf_init,
        winprelude::*,
        FileHandle,
    },
    reliable_recv_msg::{RecvResult, ReliableRecvMsg, TryRecvResult},
};
use std::{
    ffi::OsStr,
    fmt::{self, Debug, DebugStruct, Formatter},
    io::{self, prelude::*},
    marker::PhantomData,
    mem::{ManuallyDrop, MaybeUninit},
    os::windows::prelude::*,
    ptr, slice,
};
use winapi::{
    shared::winerror::ERROR_MORE_DATA,
    um::{
        namedpipeapi::DisconnectNamedPipe,
        winbase::{
            GetNamedPipeClientProcessId, GetNamedPipeClientSessionId, GetNamedPipeServerProcessId,
            GetNamedPipeServerSessionId,
        },
    },
};

/// Helper, used because `spare_capacity_mut()` on `Vec` is 1.60+. Borrows whole `Vec`, not just spare capacity.
#[inline]
pub(crate) fn vec_as_uninit(vec: &mut Vec<u8>) -> &mut [MaybeUninit<u8>] {
    let cap = vec.capacity();
    unsafe { slice::from_raw_parts_mut(vec.as_mut_ptr() as *mut MaybeUninit<u8>, cap) }
}

impl RawPipeStream {
    fn connect(pipename: &OsStr, hostname: Option<&OsStr>, read: bool, write: bool) -> io::Result<Self> {
        let path = convert_and_encode_path(pipename, hostname);
        let handle = _connect(&path, read, write, WaitTimeout::DEFAULT)?;
        Ok(Self {
            handle,
            is_server: false,
        })
    }

    fn try_recv_msg(&self, buf: &mut [MaybeUninit<u8>]) -> io::Result<TryRecvResult> {
        let mut size = 0;
        let mut fit = false;
        while size == 0 {
            size = peek_msg_len(self.handle.0)?;
            fit = buf.len() >= size;
            if fit {
                match self.handle.read(&mut buf[0..size]) {
                    // The ERROR_MORE_DATA here can only be hit if we're spinning in the loop and using the `.read()`
                    // to block until a message arrives, so that we could figure out for real if it fits or not.
                    // It doesn't mean that the message gets torn, as it normally does if the buffer given to the
                    // ReadFile call is non-zero in size.
                    Err(e) if e.raw_os_error() == Some(ERROR_MORE_DATA as _) => continue,
                    Err(e) => return Err(e),
                    Ok(nsz) => size = nsz,
                }
            } else {
                break;
            }
        }
        Ok(TryRecvResult { size, fit })
    }
    fn recv_msg(&self, buf: &mut [MaybeUninit<u8>]) -> io::Result<RecvResult> {
        let TryRecvResult { mut size, fit } = self.try_recv_msg(buf)?;
        if fit {
            Ok(RecvResult::Fit(size))
        } else {
            let mut buf = Vec::with_capacity(size);
            debug_assert!(buf.capacity() >= size);

            size = self.handle.read(vec_as_uninit(&mut buf))?;
            unsafe {
                // SAFETY: Win32 guarantees that at least this much is initialized.
                buf.set_len(size)
            };
            Ok(RecvResult::Alloc(buf))
        }
    }

    fn set_nonblocking(&self, readmode: Option<PipeMode>, nonblocking: bool) -> io::Result<()> {
        unsafe { set_nonblocking_for_stream(self.handle.0, readmode, nonblocking) }
    }
    unsafe fn try_from_raw_handle(handle: HANDLE) -> Result<Self, FromRawHandleError> {
        let is_server = is_server_from_sys(handle).map_err(|e| (FromRawHandleErrorKind::IsServerCheckFailed, e))?;
        Ok(Self {
            handle: FileHandle(handle),
            is_server,
        })
    }

    fn disconnect(&self) -> io::Result<()> {
        let success = unsafe { DisconnectNamedPipe(self.as_raw_handle()) != 0 };
        ok_or_ret_errno!(success => ())
    }

    fn fill_fields<'a, 'b, 'c>(
        &self,
        dbst: &'a mut DebugStruct<'b, 'c>,
        readmode: Option<PipeMode>,
        writemode: Option<PipeMode>,
    ) -> &'a mut DebugStruct<'b, 'c> {
        if let Some(readmode) = readmode {
            dbst.field("read_mode", &readmode);
        }
        if let Some(writemode) = writemode {
            dbst.field("write_mode", &writemode);
        }
        dbst.field("handle", &self.handle).field("is_server", &self.is_server)
    }
}
impl Drop for RawPipeStream {
    fn drop(&mut self) {
        if self.is_server {
            self.disconnect().expect("failed to disconnect server from client");
        }
    }
}
impl AsRawHandle for RawPipeStream {
    #[inline(always)]
    fn as_raw_handle(&self) -> HANDLE {
        self.handle.0
    }
}
impl IntoRawHandle for RawPipeStream {
    #[inline]
    fn into_raw_handle(self) -> HANDLE {
        let slf = ManuallyDrop::new(self);
        let handle = unsafe {
            // SAFETY: `slf` is never dropped
            ptr::read(&slf.handle)
        };
        handle.into_raw_handle()
    }
}

impl<Sm: PipeModeTag> PipeStream<pipe_mode::Messages, Sm> {
    /// Same as [`.recv()`](Self::recv), but accepts an uninitialized buffer.
    #[inline]
    pub fn recv_to_uninit(&self, buf: &mut [MaybeUninit<u8>]) -> io::Result<RecvResult> {
        self.raw.recv_msg(buf)
    }
    /// Same as [`.try_recv()`](Self::try_recv), but accepts an uninitialized buffer.
    #[inline]
    pub fn try_recv_to_uninit(&self, buf: &mut [MaybeUninit<u8>]) -> io::Result<TryRecvResult> {
        self.raw.try_recv_msg(buf)
    }
}
impl<Rm: PipeModeTag> PipeStream<Rm, pipe_mode::Messages> {
    /// Sends a message into the pipe, returning how many bytes were successfully sent (typically equal to the size of what was requested to be sent).
    #[inline]
    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.raw.handle.write(buf)
    }
}
impl<Sm: PipeModeTag> PipeStream<pipe_mode::Bytes, Sm> {
    /// Same as `.read()` from the [`Read`] trait, but accepts an uninitialized buffer.
    #[inline]
    pub fn read_to_uninit(&self, buf: &mut [MaybeUninit<u8>]) -> io::Result<usize> {
        self.raw.handle.read(buf)
    }
}
impl<Rm: PipeModeTag, Sm: PipeModeTag> PipeStream<Rm, Sm> {
    /// Connects to the specified named pipe (the `\\.\pipe\` prefix is added automatically), blocking until a server instance is dispatched.
    pub fn connect(pipename: impl AsRef<OsStr>) -> io::Result<Self> {
        let raw = RawPipeStream::connect(pipename.as_ref(), None, Rm::MODE.is_some(), Sm::MODE.is_some())?;
        Ok(Self::new(raw))
    }
    /// Connects to the specified named pipe at a remote computer (the `\\<hostname>\pipe\` prefix is added automatically), blocking until a server instance is dispatched.
    pub fn connect_to_remote(pipename: impl AsRef<OsStr>, hostname: impl AsRef<OsStr>) -> io::Result<Self> {
        let raw = RawPipeStream::connect(
            pipename.as_ref(),
            Some(hostname.as_ref()),
            Rm::MODE.is_some(),
            Sm::MODE.is_some(),
        )?;
        Ok(Self::new(raw))
    }
    /// Splits the pipe stream by value, returning a receive half and a send half. The stream is closed when both are dropped, kind of like an `Arc` (I wonder how it's implemented under the hood...).
    pub fn split(self) -> (RecvHalf<Rm>, SendHalf<Sm>) {
        let raw_a = Arc::new(self.raw);
        let raw_ac = Arc::clone(&raw_a);
        (
            RecvHalf {
                raw: raw_a,
                _phantom: PhantomData,
            },
            SendHalf {
                raw: raw_ac,
                _phantom: PhantomData,
            },
        )
    }
    /// Retrieves the process identifier of the client side of the named pipe connection.
    #[inline]
    pub fn client_process_id(&self) -> io::Result<u32> {
        unsafe { hget(self.raw.handle.0, GetNamedPipeClientProcessId) }
    }
    /// Retrieves the session identifier of the client side of the named pipe connection.
    #[inline]
    pub fn client_session_id(&self) -> io::Result<u32> {
        unsafe { hget(self.raw.handle.0, GetNamedPipeClientSessionId) }
    }
    /// Retrieves the process identifier of the server side of the named pipe connection.
    #[inline]
    pub fn server_process_id(&self) -> io::Result<u32> {
        unsafe { hget(self.raw.handle.0, GetNamedPipeServerProcessId) }
    }
    /// Retrieves the session identifier of the server side of the named pipe connection.
    #[inline]
    pub fn server_session_id(&self) -> io::Result<u32> {
        unsafe { hget(self.raw.handle.0, GetNamedPipeServerSessionId) }
    }
    /// Returns `true` if the stream was created by a listener (server-side), `false` if it was created by connecting to a server (server-side).
    #[inline]
    pub fn is_server(&self) -> bool {
        self.raw.is_server
    }
    /// Returns `true` if the stream was created by connecting to a server (client-side), `false` if it was created by a listener (server-side).
    #[inline]
    pub fn is_client(&self) -> bool {
        !self.raw.is_server
    }
    /// Sets whether the nonblocking mode for the pipe stream is enabled. By default, it is disabled.
    ///
    /// In nonblocking mode, attempts to read from the pipe when there is no data available or to write when the buffer has filled up because the receiving side did not read enough bytes in time will never block like they normally do. Instead, a [`WouldBlock`](io::ErrorKind::WouldBlock) error is immediately returned, allowing the thread to perform useful actions in the meantime.
    ///
    /// *If called on the server side, the flag will be set only for one stream instance.* A listener creation option, [`nonblocking`], and a similar method on the listener, [`set_nonblocking`], can be used to set the mode in bulk for all current instances and future ones.
    ///
    /// [`nonblocking`]: super::super::PipeListenerOptions::nonblocking
    /// [`set_nonblocking`]: super::super::PipeListenerOptions::set_nonblocking
    #[inline]
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.raw.set_nonblocking(Rm::MODE, nonblocking)
    }
    /// Attempts to wrap the given handle into the high-level pipe stream type. If the underlying pipe type is wrong or trying to figure out whether it's wrong or not caused a system call error, the corresponding error condition is returned.
    ///
    /// For more on why this can fail, see [`FromRawHandleError`]. Most notably, server-side write-only pipes will cause "access denied" errors because they lack permissions to check whether it's a server-side pipe and whether it has message boundaries.
    ///
    /// # Safety
    /// See equivalent safety notes on [`FromRawHandle`].
    pub unsafe fn from_raw_handle(handle: HANDLE) -> Result<Self, FromRawHandleError> {
        let raw = unsafe {
            // SAFETY: safety contract is propagated.
            RawPipeStream::try_from_raw_handle(handle)?
        };
        // If the wrapper type tries to read incoming data as messages, that might break if
        // the underlying pipe has no message boundaries. Let's check for that.
        if Rm::MODE == Some(PipeMode::Messages) {
            let msg_bnd = has_msg_boundaries_from_sys(raw.handle.0)
                .map_err(|e| (FromRawHandleErrorKind::MessageBoundariesCheckFailed, e))?;
            if !msg_bnd {
                return Err((
                    FromRawHandleErrorKind::NoMessageBoundaries,
                    io::Error::from(io::ErrorKind::InvalidInput),
                ));
            }
        }
        Ok(Self::new(raw))
    }

    /// Internal constructor used by the listener. It's a logic error, but not UB, to create the thing from the wrong kind of thing, but that never ever happens, to the best of my ability.
    pub(crate) fn new(raw: RawPipeStream) -> Self {
        Self {
            raw,
            _phantom: PhantomData,
        }
    }
}
impl<Rm: PipeModeTag, Sm: PipeModeTag + PmtNotNone> PipeStream<Rm, Sm> {
    /// Flushes the stream, blocking until the send buffer is empty (has been received by the other end in its entirety).
    ///
    /// Only available on streams that have a send mode.
    #[inline]
    pub fn flush(&self) -> io::Result<()> {
        self.raw.handle.flush()
    }
}
impl<Sm: PipeModeTag> Read for &PipeStream<pipe_mode::Bytes, Sm> {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.raw.handle.read(weaken_buf_init(buf))
    }
}
impl<Sm: PipeModeTag> Read for PipeStream<pipe_mode::Bytes, Sm> {
    #[inline(always)]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (self as &PipeStream<_, _>).read(buf)
    }
}
impl<Rm: PipeModeTag> Write for &PipeStream<Rm, pipe_mode::Bytes> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.raw.handle.write(buf)
    }
    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        (*self).flush()
    }
}
impl<Rm: PipeModeTag> Write for PipeStream<Rm, pipe_mode::Bytes> {
    #[inline(always)]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (self as &PipeStream<_, _>).write(buf)
    }
    #[inline(always)]
    fn flush(&mut self) -> io::Result<()> {
        (self as &PipeStream<_, _>).flush()
    }
}
impl<Sm: PipeModeTag> ReliableRecvMsg for &PipeStream<pipe_mode::Messages, Sm> {
    fn recv(&mut self, buf: &mut [u8]) -> io::Result<RecvResult> {
        self.recv_to_uninit(weaken_buf_init(buf))
    }
    fn try_recv(&mut self, buf: &mut [u8]) -> io::Result<TryRecvResult> {
        self.try_recv_to_uninit(weaken_buf_init(buf))
    }
}
impl<Sm: PipeModeTag> ReliableRecvMsg for PipeStream<pipe_mode::Messages, Sm> {
    fn recv(&mut self, buf: &mut [u8]) -> io::Result<RecvResult> {
        (self as &PipeStream<_, _>).recv(buf)
    }
    fn try_recv(&mut self, buf: &mut [u8]) -> io::Result<TryRecvResult> {
        (self as &PipeStream<_, _>).try_recv(buf)
    }
}
impl<Rm: PipeModeTag, Sm: PipeModeTag> Debug for PipeStream<Rm, Sm> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut dbst = f.debug_struct("PipeStream");
        self.raw.fill_fields(&mut dbst, Rm::MODE, Sm::MODE).finish()
    }
}
impl<Rm: PipeModeTag, Sm: PipeModeTag> AsRawHandle for PipeStream<Rm, Sm> {
    #[inline(always)]
    fn as_raw_handle(&self) -> HANDLE {
        self.raw.handle.0
    }
}
impl<Rm: PipeModeTag, Sm: PipeModeTag> IntoRawHandle for PipeStream<Rm, Sm> {
    #[inline]
    fn into_raw_handle(self) -> RawHandle {
        self.raw.into_raw_handle()
    }
}
