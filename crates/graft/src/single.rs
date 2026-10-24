//! One Graft at a time. The first Graft holds a named mutex and listens on a
//! named pipe; a second start sends its folders there, lets the first take
//! the focus, and exits.
//!
//! The mutex is per Windows session; the pipe's name adds the session too,
//! only this user may open it, and a second start only talks to a pipe whose
//! server runs as this user in this session.

use std::path::PathBuf;

/// Development builds get their own names, so `cargo run` works next to an
/// installed Graft.
const NAME: &str = if cfg!(debug_assertions) {
    "Graft-dev"
} else {
    "Graft"
};
/// No real message comes near this.
const MAX_MESSAGE: u64 = 1024 * 1024;

pub enum Start {
    /// This is the only Graft: run, and listen with [`listen`].
    First(Instance),
    /// A running Graft took over the folders; exit.
    Forwarded,
}

/// Keeps the mutex for as long as this Graft runs.
pub struct Instance {
    #[cfg(windows)]
    _mutex: win::Handle,
}

/// Become the only Graft, or hand `folders` to the one already running. If
/// that one doesn't answer (still starting, or stuck), or `forward` is
/// false (an update's restart, while the old Graft may still be closing),
/// this one runs too.
pub fn claim(folders: &[PathBuf], forward: bool) -> Start {
    #[cfg(windows)]
    {
        let (mutex, existed) = win::mutex(&format!("Local\\{NAME}-{}", user()));
        if existed && forward && win::send(&pipe_name(), &encode(folders)).is_ok() {
            return Start::Forwarded;
        }
        Start::First(Instance { _mutex: mutex })
    }
    #[cfg(not(windows))]
    {
        let _ = (folders, forward);
        Start::First(Instance {})
    }
}

/// Call `on_open` with the folders of every later start (empty: just focus),
/// on a background thread, for as long as Graft runs.
pub fn listen(on_open: impl Fn(Vec<PathBuf>) + Send + Sync + 'static) {
    #[cfg(windows)]
    {
        let name = pipe_name();
        std::thread::Builder::new()
            .name("single-instance".to_owned())
            .spawn(move || win::serve(&name, move |bytes| on_open(decode(bytes))))
            .expect("start the single-instance thread");
    }
    #[cfg(not(windows))]
    let _ = on_open;
}

fn user() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}

fn pipe_name() -> String {
    #[cfg(windows)]
    let session = win::session(std::process::id()).unwrap_or_default();
    #[cfg(not(windows))]
    let session = 0;
    format!(r"\\.\pipe\{NAME}-{}-{session}", user())
}

/// One absolute path per line; Windows paths can't contain line breaks.
fn encode(folders: &[PathBuf]) -> Vec<u8> {
    let mut out = String::new();
    for folder in folders {
        let folder = std::path::absolute(folder).unwrap_or_else(|_| folder.clone());
        out.push_str(&folder.to_string_lossy());
        out.push('\n');
    }
    out.into_bytes()
}

fn decode(bytes: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(windows)]
mod win {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::ptr::{null, null_mut};
    use std::sync::Arc;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        WaitNamedPipeW,
    };
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::{
        CreateMutexW, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    use super::MAX_MESSAGE;

    pub struct Handle(HANDLE);

    // SAFETY: a kernel handle may be used and closed from any thread.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from a Create/Open call and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(Some(0))
            .collect()
    }

    /// Open (or create) the named mutex; `true` if it existed already.
    pub fn mutex(name: &str) -> (Handle, bool) {
        let name = wide(name);
        // SAFETY: a valid, NUL-terminated name; GetLastError right after.
        unsafe {
            let handle = CreateMutexW(null(), 0, name.as_ptr());
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            (Handle(handle), existed && !handle.is_null())
        }
    }

    /// The Windows session process `pid` runs in.
    pub fn session(pid: u32) -> Option<u32> {
        let mut session = 0;
        // SAFETY: plain call with a valid out pointer.
        (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
    }

    /// The account process `pid` runs as, as a SID string ("S-1-5-21-…").
    pub fn user_sid(pid: u32) -> Option<String> {
        // SAFETY: every handle is checked and closed by `Handle`; the token
        // buffer is u64-aligned and as large as Windows asked for; the SID
        // string is freed with LocalFree as documented.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return None;
            }
            let process = Handle(process);
            let mut token = null_mut();
            if OpenProcessToken(process.0, TOKEN_QUERY, &mut token) == 0 {
                return None;
            }
            let token = Handle(token);
            let mut len = 0u32;
            GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut len);
            let mut buffer = vec![0u64; (len as usize).div_ceil(8)];
            if GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                len,
                &mut len,
            ) == 0
            {
                return None;
            }
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut text = null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return None;
            }
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            Some(sid)
        }
    }

    /// Does process `pid` run as this user, in this session?
    pub fn trusted(pid: u32) -> bool {
        let me = std::process::id();
        session(pid).is_some_and(|s| Some(s) == session(me))
            && user_sid(pid).is_some_and(|sid| Some(sid) == user_sid(me))
    }

    /// Send `message` to the Graft listening on `name`, and let it take the
    /// focus (Windows only lets the process the user just started do that).
    pub fn send(name: &str, message: &[u8]) -> std::io::Result<()> {
        let wide_name = wide(name);
        // The first Graft may be between two connections, or still starting.
        let mut pipe = None;
        for _ in 0..30 {
            match std::fs::OpenOptions::new().write(true).open(name) {
                Ok(file) => {
                    pipe = Some(file);
                    break;
                }
                Err(_) => {
                    // SAFETY: a valid, NUL-terminated name.
                    if unsafe { WaitNamedPipeW(wide_name.as_ptr(), 100) } == 0 {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        }
        let mut pipe = pipe.ok_or_else(|| std::io::Error::other("no Graft is listening"))?;
        let mut server = 0u32;
        // SAFETY: the pipe handle is open for the duration of the call.
        if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as HANDLE, &mut server) } == 0
            || !trusted(server)
        {
            return Err(std::io::Error::other(
                "the pipe belongs to another user or session",
            ));
        }
        // SAFETY: plain call; failure only means no focus change.
        unsafe { AllowSetForegroundWindow(server) };
        pipe.write_all(message)?;
        pipe.flush()
    }

    /// Only this user may open the pipe, or add instances to it.
    struct Security(PSECURITY_DESCRIPTOR);

    impl Security {
        fn own_user() -> Option<Self> {
            let sid = user_sid(std::process::id())?;
            let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
            let mut descriptor = null_mut();
            // SAFETY: a valid SDDL string; the descriptor is freed in Drop.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    null_mut(),
                )
            };
            (ok != 0).then_some(Self(descriptor))
        }
    }

    impl Drop for Security {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSecurityDescriptor…, freed once.
            unsafe { LocalFree(self.0) };
        }
    }

    /// Accept one connection after another on `name` and hand each message
    /// to `on_message`. A slow sender gets its own thread, so it can't hold
    /// up the next one. Returns if the pipe can't be had: after an update
    /// the old Graft may hold it for a few seconds, so that is waited for.
    pub fn serve(name: &str, on_message: impl Fn(&[u8]) + Send + Sync + 'static) {
        let Some(security) = Security::own_user() else {
            tracing::warn!("single instance: could not secure the pipe");
            return;
        };
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security.0,
            bInheritHandle: 0,
        };
        let on_message = Arc::new(on_message);
        let name = wide(name);
        let mut first = true;
        let mut attempts = 0;
        loop {
            // The first instance must be new, so nobody else owns the name.
            let flags = PIPE_ACCESS_INBOUND
                | if first {
                    FILE_FLAG_FIRST_PIPE_INSTANCE
                } else {
                    0
                };
            // SAFETY: a valid name and attributes; the handle is owned by
            // `File` below.
            let handle = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    flags,
                    PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    0,
                    64 * 1024,
                    0,
                    &attributes,
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                attempts += 1;
                if !first || attempts >= 60 {
                    tracing::warn!("single instance: could not create the pipe");
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            first = false;
            // SAFETY: a fresh handle, owned and closed by `pipe` from here on.
            let pipe = unsafe { File::from_raw_handle(handle as _) };
            // SAFETY: the handle is valid; no overlapped I/O.
            let connected = unsafe { ConnectNamedPipe(handle, null_mut()) } != 0
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if connected {
                let on_message = Arc::clone(&on_message);
                std::thread::spawn(move || {
                    let mut message = Vec::new();
                    if pipe.take(MAX_MESSAGE).read_to_end(&mut message).is_ok() {
                        on_message(&message);
                    }
                });
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::mpsc;

        use super::*;

        #[test]
        fn a_second_start_reaches_the_first() {
            let name = format!(r"\\.\pipe\graft-test-{}", std::process::id());
            let (tx, rx) = mpsc::channel();
            let server = name.clone();
            let tx = std::sync::Mutex::new(tx);
            std::thread::spawn(move || {
                serve(&server, move |m| {
                    tx.lock().unwrap().send(m.to_vec()).unwrap()
                })
            });

            send(&name, b"C:\\one\n").unwrap();
            let timeout = Duration::from_secs(5);
            assert_eq!(rx.recv_timeout(timeout).unwrap(), b"C:\\one\n");
            send(&name, b"").unwrap();
            assert_eq!(rx.recv_timeout(timeout).unwrap(), b"");
        }

        #[test]
        fn a_sender_that_never_finishes_holds_nobody_up() {
            let name = format!(r"\\.\pipe\graft-test-stuck-{}", std::process::id());
            let (tx, rx) = mpsc::channel();
            let server = name.clone();
            let tx = std::sync::Mutex::new(tx);
            std::thread::spawn(move || {
                serve(&server, move |m| {
                    tx.lock().unwrap().send(m.to_vec()).unwrap()
                })
            });
            send(&name, b"").unwrap();
            let timeout = Duration::from_secs(5);
            rx.recv_timeout(timeout).unwrap();

            // Connected and silent, as long as this test runs.
            let _stuck = std::fs::OpenOptions::new().write(true).open(&name).unwrap();
            send(&name, b"C:\\two\n").unwrap();
            assert_eq!(rx.recv_timeout(timeout).unwrap(), b"C:\\two\n");
        }

        #[test]
        fn only_this_user_in_this_session_is_trusted() {
            assert!(trusted(std::process::id()));
            assert!(user_sid(std::process::id()).unwrap().starts_with("S-1-"));
            // The System process: another account (and session 0).
            assert!(!trusted(4));
            assert!(!trusted(u32::MAX));
        }

        #[test]
        fn the_mutex_tells_whether_graft_runs() {
            let name = format!(r"Local\graft-test-{}", std::process::id());
            let (first, existed) = mutex(&name);
            assert!(!existed);
            let (_second, existed) = mutex(&name);
            assert!(existed);
            drop(first);
        }

        #[test]
        fn nobody_listening_is_an_error() {
            let name = format!(r"\\.\pipe\graft-test-nobody-{}", std::process::id());
            assert!(send(&name, b"").is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_survive_the_trip() {
        let folders = vec![PathBuf::from(r"C:\repos\a b"), PathBuf::from(r"D:\ünïcode")];
        assert_eq!(decode(&encode(&folders)), folders);
        assert!(decode(&encode(&[])).is_empty());
    }
}
