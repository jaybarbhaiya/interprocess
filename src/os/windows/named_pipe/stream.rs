use crate::os::windows::{
    imports::*,
    named_pipe::{PipeMode, PipeOps, PipeStreamInternals, PipeStreamRole},
    AsRawHandle, FromRawHandle, IntoRawHandle,
};
use crate::{PartialMsgWriteError, ReliableReadMsg};
use std::{
    ffi::OsStr,
    fmt::{self, Debug, Formatter},
    io::{self, Read, Write},
    mem::ManuallyDrop,
    ptr,
};

mod inst {
    use super::*;
    /// Wrapper for sync `PipeOps` to make the macro work. Will be gone soon once I redesign the API to use generics.
    pub struct Instance(PipeOps);
    impl Instance {
        pub fn create_non_taken(ops: PipeOps) -> Self {
            ops.into()
        }
        pub fn new(ops: PipeOps, _: bool) -> Self {
            ops.into()
        }
        pub fn instance(&self) -> &PipeOps {
            &self.0
        }
        pub fn is_server(&self) -> bool {
            self.0
                .is_server()
                .expect("the API desperately needs a redesign")
        }
        pub fn is_split(&self) -> bool {
            // sync pipes don't implement splitting yet
            false
        }
    }
    impl From<PipeOps> for Instance {
        fn from(x: PipeOps) -> Self {
            Self(x)
        }
    }
}
pub(super) use inst::*;

macro_rules! create_stream_type_base {
    (
        $ty:ident:
            extra_methods: {$($extra_methods:tt)*},
            doc: $doc:tt
    ) => {
        #[doc = $doc]
        pub struct $ty {
            instance: Instance,
        }
        impl $ty {
            // fn is_server(&self) -> bool and fn is_client(&self) -> bool
            // generated by downstream macros

            $($extra_methods)*

            fn ops(&self) -> &PipeOps {
                self.instance.instance()
            }
            /// Retrieves the process identifier of the client side of the named pipe connection.
            pub fn client_process_id(&self) -> io::Result<u32> {
                self.ops().get_client_process_id()
            }
            /// Retrieves the session identifier of the client side of the named pipe connection.
            pub fn client_session_id(&self) -> io::Result<u32> {
                self.ops().get_client_session_id()
            }
            /// Retrieves the process identifier of the server side of the named pipe connection.
            pub fn server_process_id(&self) -> io::Result<u32> {
                self.ops().get_server_process_id()
            }
            /// Retrieves the session identifier of the server side of the named pipe connection.
            pub fn server_session_id(&self) -> io::Result<u32> {
                self.ops().get_server_session_id()
            }
            /// Disconnects the named pipe stream without flushing buffers, causing all data in those buffers to be lost. This is much faster (and, in some case, the only finite-time way of ending things) than simply dropping the stream, since, for non-async named pipes, the `Drop` implementation flushes first.
            ///
            /// Only makes sense for server-side pipes and will return an error if called on a client stream. *For async pipe streams, this is the same as dropping the pipe.*
            pub fn disconnect_without_flushing(self) -> io::Result<()> {
                if self.is_split() {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "cannot abruptly disconnect a pipe stream which has been split",
                    ));
                }
                self.ops().disconnect()?;
                let self_ = ManuallyDrop::new(self);
                let instance = unsafe {
                    // SAFETY: ManuallyDrop is used to safely destroy the invalidated original
                    ptr::read(&self_.instance)
                };
                drop(instance);
                Ok(())
            }
            fn is_split(&self) -> bool {
                self.instance.is_split()
            }
        }
        #[doc(hidden)]
        impl crate::Sealed for $ty {}
        #[doc(hidden)]
        impl PipeStreamInternals for $ty {
            #[cfg(windows)]
            fn build(instance: Instance) -> Self {
                Self { instance }
            }
        }
        impl Drop for $ty {
            fn drop(&mut self) {
                if !self.is_split() {
                    if self.is_server() {
                        let _ = self.ops().server_drop_disconnect();
                    }
                }
            }
        }
        impl AsRawHandle for $ty {
            #[cfg(windows)]
            fn as_raw_handle(&self) -> HANDLE {
                self.ops().as_raw_handle()
            }
        }
        impl Debug for $ty {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($ty))
                    .field("handle", &self.as_raw_handle())
                    .finish()
            }
        }
    };
}

macro_rules! create_stream_type {
    (
        $ty:ident:
            desired_access: $desired_access:expr,
            role: $role:expr,
            read_mode: $read_mode:expr,
            write_mode: $write_mode:expr,
            doc: $doc:tt
    ) => {
        create_stream_type_base!(
            $ty:
            extra_methods: {
                /// Connects to the specified named pipe (the `\\.\pipe\` prefix is added automatically), blocking until a server instance is dispatched.
                pub fn connect(name: impl AsRef<OsStr>) -> io::Result<Self> {
                    Self::_connect(name.as_ref())
                }
                fn _connect(name: &OsStr) -> io::Result<Self> {
                    let pipeops = _connect(
                        name,
                        None,
                        Self::READ_MODE.is_some(),
                        Self::WRITE_MODE.is_some(),
                        WaitTimeout::DEFAULT,
                    )?;
                    Ok(Self { instance: Instance::create_non_taken(pipeops) })
                }
                /// Connects to the specified named pipe at a remote computer (the `\\<hostname>\pipe\` prefix is added automatically), blocking until a server instance is dispatched.
                pub fn connect_to_remote(pipe_name: impl AsRef<OsStr>, hostname: impl AsRef<OsStr>) -> io::Result<Self> {
                    Self::_connect_to_remote(pipe_name.as_ref(), hostname.as_ref())
                }
                fn _connect_to_remote(pipe_name: &OsStr, hostname: &OsStr) -> io::Result<Self> {
                    let pipeops = _connect(
                        pipe_name,
                        Some(hostname),
                        Self::READ_MODE.is_some(),
                        Self::WRITE_MODE.is_some(),
                        WaitTimeout::DEFAULT,
                    )?;
                    Ok(Self { instance: Instance::create_non_taken(pipeops) })
                }
                /// Sets whether the nonblocking mode for the pipe stream is enabled. By default, it is disabled.
                ///
                /// In nonblocking mode, attempts to read from the pipe when there is no data available or to write when the buffer has filled up because the receiving side did not read enough bytes in time will never block like they normally do. Instead, a [`WouldBlock`] error is immediately returned, allowing the thread to perform useful actions in the meantime.
                ///
                /// *If called on the server side, the flag will be set only for one stream instance.* A listener creation option, [`nonblocking`], and a similar method on the listener, [`set_nonblocking`], can be used to set the mode in bulk for all current instances and future ones.
                ///
                /// [`WouldBlock`]: https://doc.rust-lang.org/std/io/enum.ErrorKind.html#variant.WouldBlock " "
                /// [`nonblocking`]: struct.PipeListenerOptions.html#structfield.nonblocking " "
                /// [`set_nonblocking`]: struct.PipeListener.html#method.set_nonblocking " "
                pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
                    unsafe {
                        super::set_nonblocking_for_stream(self.as_raw_handle(), Self::READ_MODE, nonblocking)
                    }
                }
                /// Returns `true` if the stream was created by a listener (server-side), `false` if it was created by connecting to a server (server-side).
                pub fn is_server(&self) -> bool {
                    self.instance.is_server()
                }
                /// Returns `true` if the stream was created by connecting to a server (client-side), `false` if it was created by a listener (server-side).
                pub fn is_client(&self) -> bool {
                    !self.is_server()
                }
            },
            doc: $doc
        );
        impl FromRawHandle for $ty {
            #[cfg(windows)]
            unsafe fn from_raw_handle(handle: HANDLE) -> Self {
                let pipeops = unsafe {
                    // SAFETY: guaranteed via safety contract
                    PipeOps::from_raw_handle(handle)
                };

                let is_server = pipeops.is_server().expect("\
failed to determine if pipe was server-side or client-side during construction from raw handle");

                // If the wrapper type tries to read incoming data as messages, that might break if
                // the underlying pipe has no message boundaries. Let's check for that.
                if Self::READ_MODE == Some(PipeMode::Messages) {
                    let has_msg_boundaries = pipeops.does_pipe_have_message_boundaries().expect("\
failed to determine whether the pipe preserves message boundaries");
                    assert!(has_msg_boundaries, "\
stream wrapper type uses a message-based read mode, but the underlying pipe does not preserve \
message boundaries");
                }

                let instance = Instance::new(pipeops, is_server);
                Self { instance }
            }
        }
        impl IntoRawHandle for $ty {
            #[cfg(windows)]
            fn into_raw_handle(self) -> HANDLE {
                assert!(self.is_client(),
                    "cannot reclaim named pipe instance from server instancer");
                let handle = self.ops().as_raw_handle();
                handle
            }
        }
        impl PipeStream for $ty {
            const ROLE: PipeStreamRole = $role;
            const WRITE_MODE: Option<PipeMode> = $write_mode;
            const READ_MODE: Option<PipeMode> = $read_mode;
        }
    };
    ($(
        $ty:ident:
            desired_access: $desired_access:expr,
            role: $role:expr,
            read_mode: $read_mode:expr,
            write_mode: $write_mode:expr,
            doc: $doc:tt
    )+) => {
        $(create_stream_type!(
            $ty:
            desired_access: $desired_access,
            role: $role,
            read_mode: $read_mode,
            write_mode: $write_mode,
            doc: $doc
        );)+
    };
}
create_stream_type! {
    ByteReaderPipeStream:
        desired_access: GENERIC_READ,
        role: PipeStreamRole::Reader,
        read_mode: Some(PipeMode::Bytes),
        write_mode: None,
        doc: "
[Byte stream reader] for a named pipe.

Created either by using `PipeListener` or by connecting to a named pipe server.

[Byte stream reader]: https://doc.rust-lang.org/std/io/trait.Read.html
"
    ByteWriterPipeStream:
        desired_access: GENERIC_WRITE,
        role: PipeStreamRole::Writer,
        read_mode: None,
        write_mode: Some(PipeMode::Bytes),
        doc: "
[Byte stream writer] for a named pipe.

Created either by using `PipeListener` or by connecting to a named pipe server.

[Byte stream writer]: https://doc.rust-lang.org/std/io/trait.Write.html
"
    DuplexBytePipeStream:
        desired_access: GENERIC_READ | GENERIC_WRITE,
        role: PipeStreamRole::ReaderAndWriter,
        read_mode: Some(PipeMode::Bytes),
        write_mode: Some(PipeMode::Bytes),
        doc: "
Byte stream [reader] and [writer] for a named pipe.

Created either by using `PipeListener` or by connecting to a named pipe server.

[reader]: https://doc.rust-lang.org/std/io/trait.Read.html
[writer]: https://doc.rust-lang.org/std/io/trait.Write.html
"
    MsgReaderPipeStream:
        desired_access: GENERIC_READ,
        role: PipeStreamRole::Reader,
        read_mode: Some(PipeMode::Messages),
        write_mode: None,
        doc: "
[Message stream reader] for a named pipe.

Created either by using `PipeListener` or by connecting to a named pipe server.

[Message stream reader]: https://doc.rust-lang.org/std/io/trait.Read.html
"
    MsgWriterPipeStream:
        desired_access: GENERIC_WRITE,
        role: PipeStreamRole::Writer,
        read_mode: None,
        write_mode: Some(PipeMode::Messages),
        doc: "
[Message stream writer] for a named pipe.

Created either by using `PipeListener` or by connecting to a named pipe server.

[Message stream writer]: https://doc.rust-lang.org/std/io/trait.Write.html
"
    DuplexMsgPipeStream:
        desired_access: GENERIC_READ | GENERIC_WRITE,
        role: PipeStreamRole::ReaderAndWriter,
        read_mode: Some(PipeMode::Messages),
        write_mode: Some(PipeMode::Messages),
        doc: "
Message stream [reader] and [writer] for a named pipe.

Created either by using `PipeListener` or by connecting to a named pipe server.

[reader]: https://doc.rust-lang.org/std/io/trait.Read.html
[writer]: https://doc.rust-lang.org/std/io/trait.Write.html
"
}

impl Read for ByteReaderPipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.ops().read_bytes(buf)
    }
}

impl Write for ByteWriterPipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.ops().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.ops().flush()
    }
}

impl Read for DuplexBytePipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.ops().read_bytes(buf)
    }
}
impl Write for DuplexBytePipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.ops().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.ops().flush()
    }
}

impl Read for MsgReaderPipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.ops().read_bytes(buf)
    }
}
impl ReliableReadMsg for MsgReaderPipeStream {
    fn read_msg(&mut self, buf: &mut [u8]) -> io::Result<Result<usize, Vec<u8>>> {
        self.ops().read_msg(buf)
    }
    fn try_read_msg(&mut self, buf: &mut [u8]) -> io::Result<Result<usize, usize>> {
        self.ops().try_read_msg(buf)
    }
}

impl Write for MsgWriterPipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.ops().write(buf)? == buf.len() {
            Ok(buf.len())
        } else {
            Err(io::Error::new(io::ErrorKind::Other, PartialMsgWriteError))
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.ops().flush()
    }
}

impl Read for DuplexMsgPipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.ops().read_bytes(buf)
    }
}
impl ReliableReadMsg for DuplexMsgPipeStream {
    fn read_msg(&mut self, buf: &mut [u8]) -> io::Result<Result<usize, Vec<u8>>> {
        self.ops().read_msg(buf)
    }
    fn try_read_msg(&mut self, buf: &mut [u8]) -> io::Result<Result<usize, usize>> {
        self.ops().try_read_msg(buf)
    }
}
impl Write for DuplexMsgPipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.ops().write(buf)? == buf.len() {
            Ok(buf.len())
        } else {
            Err(io::Error::new(io::ErrorKind::Other, PartialMsgWriteError))
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.ops().flush()
    }
}

/// Defines the properties of pipe stream types.
///
/// ## Why there are multiple types of pipe streams
/// One of the similarities between Unix domain sockets and Windows named pipes is how both can be used in datagram mode and in byte stream mode, that is, like with sockets, Windows named pipes can both maintain the boundaries between packets or erase those boundaries — the specific behavior can be controlled both during pipe creation and during connection. The reader can still use the stream interface even if the writer maintains datagram boundaries, and vice versa: the system automatically disassembles the datagrams into a byte stream with virtually no cost.
///
/// The distinction between datagram-oriented connections and byte streams exists for symmetry with the standard library, where UDP and TCP sockets are represented by different types. The idea behind this is that by separating the two semantic types of sockets into two types, the distinction between those semantics can be enforced at compile time instead of using runtime errors to signal that, for example, a datagram read operation is attempted on a byte stream.
///
/// The fact that named pipes can have different data flow directions further increases the amount of various stream types. By restricting the implemented stream traits at compile time, named pipe streams can be used correctly in generic contexts unaware of named pipes without extra runtime checking for the correct pipe direction.
pub trait PipeStream: AsRawHandle + IntoRawHandle + FromRawHandle + PipeStreamInternals {
    /// The data stream flow direction for the pipe. See the [`PipeStreamRole`] enumeration for more on what this means.
    const ROLE: PipeStreamRole;
    /// The data stream mode for the pipe. If set to `PipeMode::Bytes`, message boundaries will broken and having `READ_MODE` at `PipeMode::Messages` would be a pipe creation error.
    ///
    /// For reader streams, this value has no meaning: if the reader stream belongs to the server (client sends data, server receives), then `READ_MODE` takes the role of this value; if the reader stream belongs to the client, there is no visible difference to how the server writes data since the client specifies its read mode itself anyway.
    const WRITE_MODE: Option<PipeMode>;
    /// The data stream mode used when reading from the pipe: if `WRITE_MODE` is `PipeMode::Messages` and `READ_MODE` is `PipeMode::Bytes`, the message boundaries will be destroyed when reading even though they are retained when written. See the `PipeMode` enumeration for more on what those modes mean.
    ///
    /// For writer streams, this value has no meaning: if the writer stream belongs to the server (server sends data, client receives), then the server doesn't read data at all and thus this does not affect anything; if the writer stream belongs to the client, then the client doesn't read anything and the value is meaningless as well.
    const READ_MODE: Option<PipeMode>;
}

/// Tries to connect to the specified named pipe (the `\\.\pipe\` prefix is added automatically), returning a named pipe stream of the stream type provided via generic parameters. If there is no available server, returns immediately.
///
/// Since named pipes can work across multiple machines, an optional hostname can be supplied. Leave it at `None` if you're using named pipes on the local machine exclusively, which is most likely the case.
#[deprecated(note = "\
poor ergonomics: you can't use turbofish syntax due to `impl AsRef<OsStr>` parameters and you \
have to use `None::<&OsStr>` instead of just `None` to provide an empty hostname")]
pub fn connect<Stream: PipeStream>(
    pipe_name: impl AsRef<OsStr>,
    hostname: Option<impl AsRef<OsStr>>,
) -> io::Result<Stream> {
    let pipeops = _connect(
        pipe_name.as_ref(),
        hostname.as_ref().map(AsRef::as_ref),
        Stream::READ_MODE.is_some(),
        Stream::WRITE_MODE.is_some(),
        WaitTimeout::DEFAULT,
    )?;
    let instance = Instance::create_non_taken(pipeops);
    Ok(Stream::build(instance))
}

fn _connect(
    pipe_name: &OsStr,
    hostname: Option<&OsStr>,
    read: bool,
    write: bool,
    timeout: WaitTimeout,
) -> io::Result<PipeOps> {
    let path = super::convert_path(pipe_name, hostname);
    loop {
        match connect_without_waiting(&path, read, write) {
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                wait_for_server(&path, timeout)?;
                continue;
            }
            els => return els,
        }
    }
}

fn connect_without_waiting(path: &[u16], read: bool, write: bool) -> io::Result<PipeOps> {
    let (success, handle) = unsafe {
        let handle = CreateFileW(
            path.as_ptr() as *mut _,
            {
                let mut access_flags: DWORD = 0;
                if read {
                    access_flags |= GENERIC_READ;
                }
                if write {
                    access_flags |= GENERIC_WRITE;
                }
                access_flags
            },
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null_mut(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        );
        (handle != INVALID_HANDLE_VALUE, handle)
    };
    if success {
        unsafe {
            // SAFETY: we just created this handle
            Ok(PipeOps::from_raw_handle(handle))
        }
    } else {
        Err(io::Error::last_os_error())
    }
}

#[repr(transparent)] // #[repr(DWORD)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct WaitTimeout(u32);
impl WaitTimeout {
    const DEFAULT: Self = Self(0x00000000);
    //const FOREVER: Self = Self(0xffffffff);
}
impl From<WaitTimeout> for u32 {
    fn from(x: WaitTimeout) -> Self {
        x.0
    }
}
impl Default for WaitTimeout {
    fn default() -> Self {
        Self::DEFAULT
    }
}
fn wait_for_server(path: &[u16], timeout: WaitTimeout) -> io::Result<()> {
    let success = unsafe { WaitNamedPipeW(path.as_ptr() as *mut _, timeout.0) != 0 };
    if success {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
