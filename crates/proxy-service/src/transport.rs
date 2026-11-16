use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::AppError;

pub trait Pipe: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Pipe for T {}
pub type Stream = Box<dyn Pipe>;

pub(crate) fn unavailable() -> AppError {
    AppError::new("worker_unavailable", "The background proxy is unavailable. Reopen settings to reconnect or check your OS background-app permissions.")
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::{
        fs::{self, File, OpenOptions},
        os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        path::PathBuf,
    };
    use tokio::net::{UnixListener, UnixStream};

    fn uid() -> u32 {
        // SAFETY: getuid has no arguments and returns the current process's real user ID.
        unsafe { libc::getuid() }
    }

    pub fn user_id() -> Result<String, AppError> {
        Ok(uid().to_string())
    }

    fn directory() -> PathBuf {
        // A short path is necessary for the macOS Unix socket path length limit.
        PathBuf::from("/tmp").join(format!("llmproxy-{}", uid()))
    }

    fn verify_directory(path: &std::path::Path) -> Result<(), AppError> {
        let metadata = fs::symlink_metadata(path).map_err(|_| unavailable())?;
        if !metadata.is_dir() || metadata.uid() != uid() || metadata.mode() & 0o077 != 0 {
            return Err(unavailable());
        }
        Ok(())
    }

    pub struct Listener {
        socket: UnixListener,
        path: PathBuf,
        _lock: File,
    }

    impl Listener {
        pub fn bind() -> Result<Self, AppError> {
            Self::bind_at(directory())
        }

        pub(super) fn bind_at(directory: PathBuf) -> Result<Self, AppError> {
            use std::os::unix::fs::DirBuilderExt;
            match fs::DirBuilder::new().mode(0o700).create(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(unavailable()),
            }
            verify_directory(&directory)?;
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(directory.join("worker.lock"))
                .map_err(|_| unavailable())?;
            let metadata = lock.metadata().map_err(|_| unavailable())?;
            if !metadata.is_file() || metadata.uid() != uid() || metadata.mode() & 0o077 != 0 {
                return Err(unavailable());
            }
            lock.try_lock().map_err(|_| unavailable())?;
            let path = directory.join("worker.sock");
            // Only the lock owner can remove a stale socket left by a crashed worker.
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(unavailable()),
            }
            let socket = UnixListener::bind(&path).map_err(|_| unavailable())?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(|_| unavailable())?;
            Ok(Self {
                socket,
                path,
                _lock: lock,
            })
        }

        pub async fn accept(&mut self) -> Result<Stream, AppError> {
            let (stream, _) = self.socket.accept().await.map_err(|_| unavailable())?;
            if stream.peer_cred().map_err(|_| unavailable())?.uid() != uid() {
                return Err(unavailable());
            }
            Ok(Box::new(stream))
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    pub async fn connect() -> Result<Stream, AppError> {
        let directory = directory();
        verify_directory(&directory)?;
        let stream = UnixStream::connect(directory.join("worker.sock"))
            .await
            .map_err(|_| unavailable())?;
        if stream.peer_cred().map_err(|_| unavailable())?.uid() != uid() {
            return Err(unavailable());
        }
        Ok(Box::new(stream))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn singleton_does_not_unlink_a_live_socket_and_recovers_after_shutdown() {
            let root = tempfile::tempdir().expect("isolated runtime directory");
            let directory = root.path().join("private");
            let first = Listener::bind_at(directory.clone()).expect("first worker");
            assert!(Listener::bind_at(directory.clone()).is_err());
            UnixStream::connect(directory.join("worker.sock"))
                .await
                .expect("original socket remains available");
            drop(first);
            let _second = Listener::bind_at(directory).expect("worker restart");
        }

        #[test]
        fn rejects_a_shared_directory_or_symlink() {
            let root = tempfile::tempdir().expect("isolated runtime directory");
            let path = root.path().join("shared");
            fs::create_dir(&path).expect("directory");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("shared permissions");
            assert!(Listener::bind_at(path.clone()).is_err());
            let link = root.path().join("link");
            std::os::unix::fs::symlink(&path, &link).expect("symlink fixture");
            assert!(Listener::bind_at(link).is_err());
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
    };
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE, WAIT_OBJECT_0},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        },
        System::{
            Pipes::GetNamedPipeServerProcessId,
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, WaitForSingleObject,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            },
        },
    };

    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            // SAFETY: this is an owned token handle returned by OpenProcessToken.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    struct Local(*mut c_void);
    impl Drop for Local {
        fn drop(&mut self) {
            // SAFETY: Windows allocated this buffer with LocalAlloc in the conversion APIs.
            unsafe {
                LocalFree(self.0);
            }
        }
    }

    pub fn user_id() -> Result<String, AppError> {
        // SAFETY: GetCurrentProcess returns a borrowed pseudo-handle valid in this process.
        sid_for_process(unsafe { GetCurrentProcess() })
    }

    fn sid_for_process(process: HANDLE) -> Result<String, AppError> {
        let mut raw = ptr::null_mut();
        // SAFETY: process is live; the output pointer is valid and requests read-only token access.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut raw) } == 0 {
            return Err(unavailable());
        }
        let token = Token(raw);
        let mut length = 0;
        // SAFETY: a null buffer queries the required size; token and length are valid.
        unsafe {
            GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut length);
        }
        if length == 0 {
            return Err(unavailable());
        }
        // usize storage provides sufficient alignment for TOKEN_USER and its trailing SID.
        let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: the aligned buffer is at least length bytes and the token is live.
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(unavailable());
        }
        let mut text = ptr::null_mut();
        // SAFETY: GetTokenInformation initialized TOKEN_USER and the SID within the live buffer.
        if unsafe {
            ConvertSidToStringSidW(
                (*(buffer.as_ptr().cast::<TOKEN_USER>())).User.Sid,
                &mut text,
            )
        } == 0
        {
            return Err(unavailable());
        }
        let _text = Local(text.cast());
        let mut count = 0;
        // SAFETY: the conversion API returns a NUL-terminated UTF-16 SID string.
        unsafe {
            while *text.add(count) != 0 {
                count += 1;
            }
            String::from_utf16(std::slice::from_raw_parts(text, count)).map_err(|_| unavailable())
        }
    }

    fn address(sid: &str) -> String {
        format!(r"\\.\pipe\llm-proxy-{sid}")
    }

    pub struct Listener {
        next: NamedPipeServer,
        sid: String,
    }

    fn create(sid: &str, first: bool) -> Result<NamedPipeServer, AppError> {
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})\0").encode_utf16().collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: sddl is NUL-terminated, revision 1 is SDDL_REVISION_1, output pointer is valid.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(unavailable());
        }
        let _descriptor = Local(descriptor);
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: attributes and its current-user-only DACL remain alive during CreateNamedPipe.
        // Remote clients are rejected; no Everyone, administrator-group, or anonymous ACE is added.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(
                    address(sid),
                    (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
                )
        }
        .map_err(|_| unavailable())
    }

    impl Listener {
        pub fn bind() -> Result<Self, AppError> {
            let sid = user_id()?;
            Ok(Self {
                next: create(&sid, true)?,
                sid,
            })
        }

        pub async fn accept(&mut self) -> Result<Stream, AppError> {
            self.next.connect().await.map_err(|_| unavailable())?;
            // Keep an instance alive while constructing its successor so another worker cannot race in.
            let next = create(&self.sid, false)?;
            Ok(Box::new(std::mem::replace(&mut self.next, next)))
        }
    }

    pub fn open_process_for_wait(pid: u32) -> Result<OwnedHandle, AppError> {
        // SAFETY: request only wait/read access to the known worker before asking it to exit.
        let raw = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return Err(unavailable());
        }
        // SAFETY: OpenProcess returned a new owned handle, transferred exactly once.
        let process = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
        if sid_for_process(process.as_raw_handle().cast())? != user_id()? {
            return Err(unavailable());
        }
        Ok(process)
    }

    pub async fn wait_process(process: OwnedHandle) -> Result<(), AppError> {
        tokio::task::spawn_blocking(move || {
            // SAFETY: process owns a live waitable process handle for the entire call.
            if unsafe { WaitForSingleObject(process.as_raw_handle().cast(), 10_000) }
                == WAIT_OBJECT_0
            {
                Ok(())
            } else {
                Err(unavailable())
            }
        })
        .await
        .map_err(|_| unavailable())?
    }

    pub async fn connect() -> Result<Stream, AppError> {
        let sid = user_id()?;
        let pipe = ClientOptions::new()
            .open(address(&sid))
            .map_err(|_| unavailable())?;
        let mut pid = 0;
        // SAFETY: the pipe is connected and the process-ID output pointer is valid.
        if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle().cast(), &mut pid) } == 0 {
            return Err(unavailable());
        }
        // SAFETY: requesting read-only information about the live server process.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            return Err(unavailable());
        }
        let process = Token(raw);
        if sid_for_process(process.0)? != sid {
            return Err(unavailable());
        }
        Ok(Box::new(pipe))
    }
}

pub use platform::{connect, user_id, Listener};

#[cfg(windows)]
pub use platform::{open_process_for_wait, wait_process};
