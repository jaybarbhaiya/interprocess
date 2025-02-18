mod read_half;
pub use read_half::*;

mod write_half;
pub use write_half::*;

use {
    super::super::ToLocalSocketName,
    futures_io::{AsyncRead, AsyncWrite},
    std::{
        fmt::{self, Debug, Formatter},
        io::{self, IoSlice, IoSliceMut},
        pin::Pin,
        task::{Context, Poll},
    },
};

impmod! {local_socket::tokio,
    LocalSocketStream as LocalSocketStreamImpl
}

/// A Tokio-based local socket byte stream, obtained eiter from [`LocalSocketListener`](super::LocalSocketListener) or by connecting to an existing local socket.
///
/// # Examples
///
/// ## Basic client
/// ```no_run
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use futures::{
///     io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
///     try_join,
/// };
/// use interprocess::local_socket::{tokio::LocalSocketStream, NameTypeSupport};
///
/// // Pick a name. There isn't a helper function for this, mostly because it's largely unnecessary:
/// // in Rust, `match` is your concise, readable and expressive decision making construct.
/// let name = {
///     // This scoping trick allows us to nicely contain the import inside the `match`, so that if
///     // any imports of variants named `Both` happen down the line, they won't collide with the
///     // enum we're working with here. Maybe someone should make a macro for this.
///     use NameTypeSupport::*;
///     match NameTypeSupport::query() {
///         OnlyPaths => "/tmp/example.sock",
///         OnlyNamespaced | Both => "@example.sock",
///     }
/// };
///
/// // Await this here since we can't do a whole lot without a connection.
/// let conn = LocalSocketStream::connect(name).await?;
///
/// // This consumes our connection and splits it into two halves,
/// // so that we could concurrently act on both.
/// let (reader, mut writer) = conn.into_split();
/// let mut reader = BufReader::new(reader);
///
/// // Allocate a sizeable buffer for reading.
/// // This size should be enough and should be easy to find for the allocator.
/// let mut buffer = String::with_capacity(128);
///
/// // Describe the write operation as writing our whole string.
/// let write = writer.write_all(b"Hello from client!\n");
/// // Describe the read operation as reading until a newline into our buffer.
/// let read = reader.read_line(&mut buffer);
///
/// // Concurrently perform both operations.
/// try_join!(write, read)?;
///
/// // Close the connection a bit earlier than you'd think we would. Nice practice!
/// drop((reader, writer));
///
/// // Display the results when we're done!
/// println!("Server answered: {}", buffer.trim());
/// # Ok(()) }
/// ```
pub struct LocalSocketStream {
    pub(super) inner: LocalSocketStreamImpl,
}
impl LocalSocketStream {
    /// Connects to a remote local socket server.
    #[inline]
    pub async fn connect<'a>(name: impl ToLocalSocketName<'a>) -> io::Result<Self> {
        LocalSocketStreamImpl::connect(name).await.map(Self::from)
    }
    /// Splits a stream into a read half and a write half, which can be used to read and write the stream concurrently.
    #[inline]
    pub fn into_split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
        let (r, w) = self.inner.into_split();
        (OwnedReadHalf { inner: r }, OwnedWriteHalf { inner: w })
    }
    /// Retrieves the identifier of the process on the opposite end of the local socket connection.
    ///
    /// # Platform-specific behavior
    /// ## macOS and iOS
    /// Not supported by the OS, will always generate an error at runtime.
    #[inline]
    pub fn peer_pid(&self) -> io::Result<u32> {
        self.inner.peer_pid()
    }
    /// Creates a Tokio-based async object from a given raw file descriptor. This will also attach the object to the Tokio runtime this function is called in, so calling it outside a runtime will result in an error (which is why the `FromRawFd` trait can't be implemented instead).
    ///
    /// # Safety
    /// The given file descriptor must be valid (i.e. refer to an existing kernel object) and must not be owned by any other file descriptor container. If this is not upheld, an arbitrary file descriptor will be closed when the returned object is dropped.
    #[cfg(unix)]
    #[cfg_attr(feature = "doc_cfg", doc(cfg(unix)))]
    #[inline]
    pub unsafe fn from_raw_fd(fd: libc::c_int) -> io::Result<Self> {
        unsafe { LocalSocketStreamImpl::from_raw_fd(fd) }.map(Self::from)
    }
    /// Releases ownership of the raw file descriptor, detaches the object from the Tokio runtime (therefore has to be called within the runtime) and returns the file descriptor as an integer.
    #[cfg(unix)]
    #[cfg_attr(feature = "doc_cfg", doc(cfg(unix)))]
    #[inline]
    pub fn into_raw_fd(self) -> io::Result<libc::c_int> {
        self.inner.into_raw_fd()
    }
    #[inline]
    fn pinproj(&mut self) -> Pin<&mut LocalSocketStreamImpl> {
        Pin::new(&mut self.inner)
    }
}
#[doc(hidden)]
impl From<LocalSocketStreamImpl> for LocalSocketStream {
    #[inline]
    fn from(inner: LocalSocketStreamImpl) -> Self {
        Self { inner }
    }
}

impl AsyncRead for LocalSocketStream {
    #[inline]
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        self.pinproj().poll_read(cx, buf)
    }
    #[inline]
    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        self.pinproj().poll_read_vectored(cx, bufs)
    }
}
impl AsyncWrite for LocalSocketStream {
    #[inline]
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.pinproj().poll_write(cx, buf)
    }
    #[inline]
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.pinproj().poll_write_vectored(cx, bufs)
    }
    // Those don't do anything
    #[inline]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.pinproj().poll_flush(cx)
    }
    #[inline]
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.pinproj().poll_close(cx)
    }
}

impl Debug for LocalSocketStream {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.inner, f)
    }
}

impl_as_raw_handle!(LocalSocketStream);
