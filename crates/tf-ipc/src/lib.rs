//! IPC transport layer for TermForge.
//!
//! Provides a unified abstraction over Windows named pipes and Unix domain sockets
//! with token-based authentication per ADR 0006.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use tf_proto::{write_frame, ClientMsg, FrameError, ProtoError, ServerMsg, MAX_FRAME_LEN};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{error, info, warn};

const MAX_SERVER_CONNECTIONS: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
    #[error("frame: {0}")]
    Frame(#[from] FrameError),
    #[error("authentication failed")]
    Unauthorized,
    #[error("server not running")]
    NotRunning,
    #[error("connection closed")]
    Closed,
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtoError),
}

pub type Result<T> = std::result::Result<T, IpcError>;

/// Server-side connection: receives ClientMsg, sends ServerMsg.
#[async_trait]
pub trait ServerConnection: Send + Sync {
    async fn send(&mut self, msg: ServerMsg) -> Result<()>;
    async fn recv(&mut self) -> Result<Option<ClientMsg>>;
    async fn close(&mut self) -> Result<()>;
}

/// Client-side connection: sends ClientMsg, receives ServerMsg.
#[async_trait]
pub trait ClientConnection: Send + Sync {
    async fn send(&mut self, msg: ClientMsg) -> Result<()>;
    async fn recv(&mut self) -> Result<Option<ServerMsg>>;
    async fn close(&mut self) -> Result<()>;
}

/// Handler for incoming server connections.
#[async_trait]
pub trait ConnectionHandler: Send + Sync {
    async fn handle(&self, conn: Box<dyn ServerConnection>) -> Result<()>;
}

/// IPC server trait.
#[async_trait]
pub trait IpcServer: Send + Sync {
    async fn start(&mut self, handler: Arc<dyn ConnectionHandler>) -> Result<()>;
    async fn stop(&mut self) -> Result<()>;
    fn address(&self) -> &str;
}

/// Client for connecting to the IPC server.
pub struct IpcClient {
    conn: Box<dyn ClientConnection>,
    pending_events: VecDeque<tf_proto::Event>,
}

impl std::fmt::Debug for IpcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcClient").finish_non_exhaustive()
    }
}

impl IpcClient {
    /// Connect to the server with token authentication.
    pub async fn connect(address: &str, token: &str, client_name: &str) -> Result<Self> {
        let mut conn = connect_transport(address).await?;

        // Send Hello
        let hello = ClientMsg::Hello {
            version: tf_proto::PROTOCOL_VERSION,
            token: token.to_string(),
            client: client_name.to_string(),
        };
        conn.send(hello).await?;

        // Wait for Welcome
        match conn.recv().await? {
            Some(ServerMsg::Welcome { version, server }) => {
                if version != tf_proto::PROTOCOL_VERSION {
                    return Err(IpcError::Protocol(ProtoError::VersionMismatch {
                        server: version,
                        client: tf_proto::PROTOCOL_VERSION,
                    }));
                }
                info!("connected to {} (protocol v{})", server, version);
                Ok(Self {
                    conn,
                    pending_events: VecDeque::new(),
                })
            }
            Some(ServerMsg::Response {
                result: Err(ProtoError::Unauthorized),
                ..
            }) => Err(IpcError::Unauthorized),
            Some(other) => Err(IpcError::Protocol(ProtoError::Internal(format!(
                "expected welcome, got {:?}",
                other
            )))),
            None => Err(IpcError::Closed),
        }
    }

    /// Send a request and wait for the response.
    pub async fn request(&mut self, request: tf_proto::Request) -> Result<tf_proto::Response> {
        let id = rand::random::<u64>();
        self.conn.send(ClientMsg::Request { id, request }).await?;

        loop {
            match self.conn.recv().await? {
                Some(ServerMsg::Response {
                    id: resp_id,
                    result,
                }) if resp_id == id => {
                    return result.map_err(IpcError::Protocol);
                }
                Some(ServerMsg::Event(event)) => self.pending_events.push_back(event),
                Some(other) => {
                    warn!("unexpected message: {:?}", other);
                }
                None => return Err(IpcError::Closed),
            }
        }
    }

    /// Subscribe to events for a session.
    pub async fn subscribe(&mut self, session_id: tf_proto::SessionId) -> Result<()> {
        self.conn
            .send(ClientMsg::Request {
                id: rand::random(),
                request: tf_proto::Request::Subscribe {
                    session: session_id,
                },
            })
            .await?;
        // Expect OK response
        match self.conn.recv().await? {
            Some(ServerMsg::Response {
                result: Ok(tf_proto::Response::Ok),
                ..
            }) => Ok(()),
            Some(ServerMsg::Response { result: Err(e), .. }) => Err(IpcError::Protocol(e)),
            _ => Err(IpcError::Protocol(ProtoError::Internal(
                "expected OK".into(),
            ))),
        }
    }

    /// Get the next event from the server.
    pub async fn next_event(&mut self) -> Result<Option<tf_proto::Event>> {
        if let Some(event) = self.pending_events.pop_front() {
            return Ok(Some(event));
        }
        loop {
            match self.conn.recv().await? {
                Some(ServerMsg::Event(event)) => return Ok(Some(event)),
                Some(ServerMsg::Response { .. }) => {
                    // Responses to requests we didn't make; ignore
                }
                None => return Ok(None),
                _ => {}
            }
        }
    }

    pub async fn close(&mut self) -> Result<()> {
        self.conn.close().await
    }
}

// Platform-specific implementations

#[cfg(windows)]
mod windows {
    use super::*;
    use ::windows::core::{PCWSTR, PWSTR};
    use ::windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use ::windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use ::windows::Win32::Security::{
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    };
    use ::windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };
    use tokio::sync::Semaphore;

    pub const PIPE_PREFIX: &str = r"\\.\pipe\termforge-";

    struct TokenHandle(HANDLE);

    impl Drop for TokenHandle {
        fn drop(&mut self) {
            // SAFETY: OpenProcessToken returned this owned handle and it is closed once here.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

    impl Drop for SecurityDescriptor {
        fn drop(&mut self) {
            // SAFETY: The descriptor was allocated by LocalAlloc inside the conversion API.
            unsafe {
                let _ = LocalFree(HLOCAL(self.0 .0));
            }
        }
    }

    fn user_sid() -> Result<String> {
        // SAFETY: Every pointer passed to Win32 refers to live, suitably aligned storage;
        // return values and reported sizes are checked before any pointer is dereferenced.
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(win_error)?;
            let token = TokenHandle(token);

            let mut size = 0u32;
            let probe = GetTokenInformation(token.0, TokenUser, None, 0, &mut size);
            if size < size_of::<TOKEN_USER>() as u32 {
                return Err(IpcError::Io(io::Error::other(format!(
                    "query token user size failed: {}",
                    probe.expect_err("a zero-length token query must request a buffer")
                ))));
            }
            let words = (size as usize).div_ceil(size_of::<usize>());
            let mut buf = vec![0usize; words];
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(buf.as_mut_ptr() as *mut _),
                size,
                &mut size,
            )
            .map_err(win_error)?;
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let sid = user.User.Sid;
            let mut sid_str = PWSTR::null();
            ConvertSidToStringSidW(sid, &mut sid_str).map_err(win_error)?;
            let result = sid_str.to_string();
            let _ = LocalFree(HLOCAL(sid_str.0.cast()));
            result.map_err(|error| IpcError::Io(io::Error::other(error)))
        }
    }

    fn user_only_descriptor() -> Result<SecurityDescriptor> {
        let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", user_sid()?);
        let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `wide` is NUL-terminated and remains alive for the call; the returned
        // descriptor is owned by SecurityDescriptor and freed with LocalFree.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .map_err(win_error)?;
        }
        Ok(SecurityDescriptor(descriptor))
    }

    fn create_pipe(name: &str, first: bool) -> Result<NamedPipeServer> {
        let descriptor = user_only_descriptor()?;
        let mut attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0 .0,
            bInheritHandle: false.into(),
        };
        let mut opts = ServerOptions::new();
        opts.access_inbound(true)
            .access_outbound(true)
            .reject_remote_clients(true);
        if first {
            opts.first_pipe_instance(true);
        }
        // SAFETY: attrs and its security descriptor remain live for the duration of
        // CreateNamedPipeW; Windows copies the descriptor into the created object.
        unsafe {
            opts.create_with_security_attributes_raw(
                name,
                (&mut attrs as *mut SECURITY_ATTRIBUTES).cast(),
            )
            .map_err(IpcError::Io)
        }
    }

    fn pipe_name(channel: &str) -> Result<String> {
        Ok(format!("{}{}-{}", PIPE_PREFIX, user_sid()?, channel))
    }

    #[derive(Debug)]
    pub struct WindowsPipeServer {
        pipe_name: String,
        listener: Option<NamedPipeServer>,
    }

    impl WindowsPipeServer {
        pub fn new(channel: &str) -> Result<Self> {
            Ok(Self {
                pipe_name: pipe_name(channel)?,
                listener: None,
            })
        }
    }

    #[async_trait]
    impl IpcServer for WindowsPipeServer {
        async fn start(&mut self, handler: Arc<dyn ConnectionHandler>) -> Result<()> {
            let pipe_name = self.pipe_name.clone();
            let mut listener = Some(create_pipe(&pipe_name, true)?);

            let handler = handler.clone();
            let permits = Arc::new(Semaphore::new(MAX_SERVER_CONNECTIONS));
            tokio::spawn(async move {
                loop {
                    let current = match listener.take() {
                        Some(l) => l,
                        None => break,
                    };
                    match current.connect().await {
                        Ok(()) => {
                            listener = match create_pipe(&pipe_name, false) {
                                Ok(next) => Some(next),
                                Err(error) => {
                                    error!("create pipe instance: {error}");
                                    break;
                                }
                            };
                            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                                warn!("rejecting IPC connection: connection limit reached");
                                continue;
                            };
                            let handler = handler.clone();
                            tokio::spawn(async move {
                                let _permit = permit;
                                let conn = WindowsPipeServerConnection::new(current);
                                if let Err(e) = handler.handle(Box::new(conn)).await {
                                    warn!("connection error: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            error!("accept error: {e}");
                            break;
                        }
                    }
                }
            });
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.listener = None;
            Ok(())
        }

        fn address(&self) -> &str {
            &self.pipe_name
        }
    }

    pub struct WindowsPipeServerConnection {
        stream: NamedPipeServer,
        read_buf: BytesMut,
    }

    impl WindowsPipeServerConnection {
        fn new(stream: NamedPipeServer) -> Self {
            Self {
                stream,
                read_buf: BytesMut::with_capacity(4096),
            }
        }
    }

    #[async_trait]
    impl ServerConnection for WindowsPipeServerConnection {
        async fn send(&mut self, msg: ServerMsg) -> Result<()> {
            let mut buf = Vec::new();
            write_frame(&mut buf, &msg).map_err(IpcError::Frame)?;
            self.stream.write_all(&buf).await.map_err(IpcError::Io)?;
            Ok(())
        }

        async fn recv(&mut self) -> Result<Option<ClientMsg>> {
            loop {
                if self.read_buf.len() >= 4 {
                    let len = u32::from_le_bytes([
                        self.read_buf[0],
                        self.read_buf[1],
                        self.read_buf[2],
                        self.read_buf[3],
                    ]) as usize;
                    if len > MAX_FRAME_LEN {
                        return Err(IpcError::Frame(FrameError::TooLarge(len)));
                    }
                    if self.read_buf.len() >= 4 + len {
                        let payload = self.read_buf.split_to(4 + len);
                        let msg = postcard::from_bytes(&payload[4..])
                            .map_err(|e| IpcError::Frame(FrameError::Codec(e)))?;
                        return Ok(Some(msg));
                    }
                }
                self.read_buf.reserve(4096);
                let n = self
                    .stream
                    .read_buf(&mut self.read_buf)
                    .await
                    .map_err(IpcError::Io)?;
                if n == 0 {
                    return Ok(None);
                }
            }
        }

        async fn close(&mut self) -> Result<()> {
            self.stream.shutdown().await.map_err(IpcError::Io)?;
            Ok(())
        }
    }

    pub struct WindowsPipeClientConnection {
        stream: NamedPipeClient,
        read_buf: BytesMut,
    }

    impl WindowsPipeClientConnection {
        fn new(stream: NamedPipeClient) -> Self {
            Self {
                stream,
                read_buf: BytesMut::with_capacity(4096),
            }
        }
    }

    #[async_trait]
    impl ClientConnection for WindowsPipeClientConnection {
        async fn send(&mut self, msg: ClientMsg) -> Result<()> {
            let mut buf = Vec::new();
            write_frame(&mut buf, &msg).map_err(IpcError::Frame)?;
            self.stream.write_all(&buf).await.map_err(IpcError::Io)?;
            Ok(())
        }

        async fn recv(&mut self) -> Result<Option<ServerMsg>> {
            loop {
                if self.read_buf.len() >= 4 {
                    let len = u32::from_le_bytes([
                        self.read_buf[0],
                        self.read_buf[1],
                        self.read_buf[2],
                        self.read_buf[3],
                    ]) as usize;
                    if len > MAX_FRAME_LEN {
                        return Err(IpcError::Frame(FrameError::TooLarge(len)));
                    }
                    if self.read_buf.len() >= 4 + len {
                        let payload = self.read_buf.split_to(4 + len);
                        let msg = postcard::from_bytes(&payload[4..])
                            .map_err(|e| IpcError::Frame(FrameError::Codec(e)))?;
                        return Ok(Some(msg));
                    }
                }
                self.read_buf.reserve(4096);
                let n = self
                    .stream
                    .read_buf(&mut self.read_buf)
                    .await
                    .map_err(IpcError::Io)?;
                if n == 0 {
                    return Ok(None);
                }
            }
        }

        async fn close(&mut self) -> Result<()> {
            self.stream.shutdown().await.map_err(IpcError::Io)?;
            Ok(())
        }
    }

    pub async fn connect_transport(address: &str) -> Result<Box<dyn ClientConnection>> {
        let mut opts = ClientOptions::new();
        opts.read(true).write(true);
        let address = if address.is_empty() {
            pipe_name("main")?
        } else {
            address.to_owned()
        };
        let stream = opts.open(&address).map_err(IpcError::Io)?;
        Ok(Box::new(WindowsPipeClientConnection::new(stream)))
    }

    pub fn restrict_file_to_user(path: &std::path::Path) -> io::Result<()> {
        use ::windows::Win32::Security::{
            SetFileSecurityW, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        };

        let descriptor = user_only_descriptor().map_err(io::Error::other)?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: path is NUL-terminated and descriptor remains live for the call.
        unsafe {
            SetFileSecurityW(
                PCWSTR(wide.as_ptr()),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor.0,
            )
            .ok()
            .map_err(io::Error::other)
        }
    }

    fn win_error(error: ::windows::core::Error) -> IpcError {
        IpcError::Io(io::Error::other(error))
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::path::PathBuf;
    use tokio::net::UnixListener;
    use tokio::net::UnixStream;
    use tokio::sync::Semaphore;

    const SOCKET_DIR: &str = "termforge";
    const SOCKET_NAME: &str = "ipc.sock";

    fn socket_path() -> Result<PathBuf> {
        let runtime = dirs::runtime_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(SOCKET_DIR);
        std::fs::create_dir_all(&runtime)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))?;
        Ok(runtime.join(SOCKET_NAME))
    }

    #[derive(Debug)]
    pub struct UnixSocketServer {
        path: PathBuf,
        listener: Option<UnixListener>,
    }

    impl UnixSocketServer {
        pub fn new() -> Result<Self> {
            Ok(Self {
                path: socket_path()?,
                listener: None,
            })
        }
    }

    #[async_trait]
    impl IpcServer for UnixSocketServer {
        async fn start(&mut self, handler: Arc<dyn ConnectionHandler>) -> Result<()> {
            if self.path.exists() {
                if UnixStream::connect(&self.path).await.is_ok() {
                    return Err(IpcError::Io(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        format!("forged is already listening at {}", self.path.display()),
                    )));
                }
                std::fs::remove_file(&self.path)?;
            }
            let listener = UnixListener::bind(&self.path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
            }

            let handler = handler.clone();
            let path = self.path.clone();
            let mut listener = Some(listener);
            let permits = Arc::new(Semaphore::new(MAX_SERVER_CONNECTIONS));
            tokio::spawn(async move {
                while let Some(listener) = &mut listener {
                    match listener.accept().await {
                        Ok((stream, _)) => {
                            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                                warn!("rejecting IPC connection: connection limit reached");
                                continue;
                            };
                            let handler = handler.clone();
                            tokio::spawn(async move {
                                let _permit = permit;
                                let conn = UnixSocketConnection::new(stream);
                                if let Err(e) = handler.handle(Box::new(conn)).await {
                                    warn!("connection error: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            error!("accept error: {e}");
                            break;
                        }
                    }
                }
                let _ = std::fs::remove_file(&path);
            });
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.listener = None;
            let _ = std::fs::remove_file(&self.path);
            Ok(())
        }

        fn address(&self) -> &str {
            self.path.to_str().unwrap_or("")
        }
    }

    pub struct UnixSocketConnection {
        stream: UnixStream,
        read_buf: BytesMut,
    }

    impl UnixSocketConnection {
        fn new(stream: UnixStream) -> Self {
            Self {
                stream,
                read_buf: BytesMut::with_capacity(4096),
            }
        }
    }

    #[async_trait]
    impl ServerConnection for UnixSocketConnection {
        async fn send(&mut self, msg: ServerMsg) -> Result<()> {
            let mut buf = Vec::new();
            write_frame(&mut buf, &msg).map_err(IpcError::Frame)?;
            self.stream.write_all(&buf).await.map_err(IpcError::Io)?;
            Ok(())
        }

        async fn recv(&mut self) -> Result<Option<ClientMsg>> {
            loop {
                if self.read_buf.len() >= 4 {
                    let len = u32::from_le_bytes([
                        self.read_buf[0],
                        self.read_buf[1],
                        self.read_buf[2],
                        self.read_buf[3],
                    ]) as usize;
                    if len > MAX_FRAME_LEN {
                        return Err(IpcError::Frame(FrameError::TooLarge(len)));
                    }
                    if self.read_buf.len() >= 4 + len {
                        let payload = self.read_buf.split_to(4 + len);
                        let msg = postcard::from_bytes(&payload[4..])
                            .map_err(|e| IpcError::Frame(FrameError::Codec(e)))?;
                        return Ok(Some(msg));
                    }
                }
                self.read_buf.reserve(4096);
                let n = self
                    .stream
                    .read_buf(&mut self.read_buf)
                    .await
                    .map_err(IpcError::Io)?;
                if n == 0 {
                    return Ok(None);
                }
            }
        }

        async fn close(&mut self) -> Result<()> {
            self.stream.shutdown().await.map_err(IpcError::Io)?;
            Ok(())
        }
    }

    #[async_trait]
    impl ClientConnection for UnixSocketConnection {
        async fn send(&mut self, msg: ClientMsg) -> Result<()> {
            let mut buf = Vec::new();
            write_frame(&mut buf, &msg).map_err(IpcError::Frame)?;
            self.stream.write_all(&buf).await.map_err(IpcError::Io)?;
            Ok(())
        }

        async fn recv(&mut self) -> Result<Option<ServerMsg>> {
            loop {
                if self.read_buf.len() >= 4 {
                    let len = u32::from_le_bytes([
                        self.read_buf[0],
                        self.read_buf[1],
                        self.read_buf[2],
                        self.read_buf[3],
                    ]) as usize;
                    if len > MAX_FRAME_LEN {
                        return Err(IpcError::Frame(FrameError::TooLarge(len)));
                    }
                    if self.read_buf.len() >= 4 + len {
                        let payload = self.read_buf.split_to(4 + len);
                        let msg = postcard::from_bytes(&payload[4..])
                            .map_err(|e| IpcError::Frame(FrameError::Codec(e)))?;
                        return Ok(Some(msg));
                    }
                }
                self.read_buf.reserve(4096);
                let n = self
                    .stream
                    .read_buf(&mut self.read_buf)
                    .await
                    .map_err(IpcError::Io)?;
                if n == 0 {
                    return Ok(None);
                }
            }
        }

        async fn close(&mut self) -> Result<()> {
            self.stream.shutdown().await.map_err(IpcError::Io)?;
            Ok(())
        }
    }

    pub async fn connect_transport(address: &str) -> Result<Box<dyn ClientConnection>> {
        let path = if address.is_empty() {
            socket_path()?
        } else {
            PathBuf::from(address)
        };
        let stream = UnixStream::connect(path).await.map_err(IpcError::Io)?;
        Ok(Box::new(UnixSocketConnection::new(stream)))
    }
}

// Platform-agnostic exports

#[cfg(windows)]
pub use windows::{connect_transport, WindowsPipeServer as IpcServerImpl};

#[cfg(unix)]
pub use unix::{connect_transport, UnixSocketServer as IpcServerImpl};

#[cfg(unix)]
pub fn server() -> Result<IpcServerImpl> {
    IpcServerImpl::new()
}

#[cfg(windows)]
pub fn server() -> Result<IpcServerImpl> {
    IpcServerImpl::new("main")
}

/// Token management for authentication.
pub mod token {
    use std::fs;
    use std::path::PathBuf;

    pub fn token_path() -> Option<PathBuf> {
        dirs::data_local_dir().map(|d| d.join("TermForge").join("ipc.token"))
    }

    pub fn generate_token() -> String {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    pub fn read_token() -> Option<String> {
        token_path()
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|s| s.trim().to_string())
    }

    pub fn write_token(token: &str) -> std::io::Result<()> {
        if let Some(path) = token_path() {
            let parent = path.parent().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "token path has no parent")
            })?;
            fs::create_dir_all(parent)?;
            #[cfg(windows)]
            super::windows::restrict_file_to_user(parent)?;
            fs::write(&path, token)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            }
            #[cfg(windows)]
            super::windows::restrict_file_to_user(&path)?;
        }
        Ok(())
    }

    pub fn ensure_token() -> std::io::Result<String> {
        if let Some(token) = read_token() {
            if let Some(path) = token_path() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                }
                #[cfg(windows)]
                {
                    if let Some(parent) = path.parent() {
                        super::windows::restrict_file_to_user(parent)?;
                    }
                    super::windows::restrict_file_to_user(&path)?;
                }
            }
            Ok(token)
        } else {
            let token = generate_token();
            write_token(&token)?;
            Ok(token)
        }
    }
}
