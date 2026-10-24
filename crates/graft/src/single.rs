//! One Graft at a time. The first Graft holds a named mutex and listens on a
//! named pipe; a second start sends its folders there, lets the first take
//! the focus, and exits.

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
/// that one doesn't answer (still starting, or stuck), this one runs too.
pub fn claim(folders: &[PathBuf]) -> Start {
    #[cfg(windows)]
    {
        let (mutex, existed) = win::mutex(&format!("Local\\{NAME}-{}", user()));
        if existed && win::send(&pipe_name(), &encode(folders)).is_ok() {
            return Start::Forwarded;
        }
        Start::First(Instance { _mutex: mutex })
    }
    #[cfg(not(windows))]
    {
        let _ = folders;
        Start::First(Instance {})
    }
}

/// Call `on_open` with the folders of every later start (empty: just focus),
/// on a background thread, for as long as Graft runs.
pub fn listen(on_open: impl Fn(Vec<PathBuf>) + Send + 'static) {
    #[cfg(windows)]
    {
        let name = pipe_name();
        std::thread::Builder::new()
            .name("single-instance".to_owned())
            .spawn(move || win::serve(&name, |bytes| on_open(decode(bytes))))
            .expect("start the single-instance thread");
    }
    #[cfg(not(windows))]
    let _ = on_open;
}

fn user() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}

fn pipe_name() -> String {
    format!(r"\\.\pipe\{NAME}-{}", user())
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

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    use super::MAX_MESSAGE;

    pub struct Handle(HANDLE);

    // SAFETY: a kernel handle may be used and closed from any thread.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateMutexW and is closed once.
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
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            (Handle(handle), existed && !handle.is_null())
        }
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
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        }
        let mut pipe = pipe.ok_or_else(|| std::io::Error::other("no Graft is listening"))?;
        let mut server = 0u32;
        // SAFETY: the pipe handle is open for the duration of the call.
        if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as HANDLE, &mut server) } != 0
        {
            // SAFETY: plain call; failure only means no focus change.
            unsafe { AllowSetForegroundWindow(server) };
        }
        pipe.write_all(message)?;
        pipe.flush()
    }

    /// Accept one connection after another on `name` and hand each message
    /// to `on_message`. Returns if the pipe can't be created, e.g. because
    /// another process already owns the name.
    pub fn serve(name: &str, on_message: impl Fn(&[u8])) {
        let name = wide(name);
        loop {
            // SAFETY: a valid name; the handle is owned by `File` below.
            let handle = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    // Only ever one instance, created by us: nobody can
                    // squat the name and read what is sent.
                    PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    0,
                    64 * 1024,
                    0,
                    std::ptr::null(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                tracing::warn!("single instance: could not create the pipe");
                return;
            }
            // SAFETY: a fresh handle, owned and closed by `pipe` from here on.
            let pipe = unsafe { File::from_raw_handle(handle as _) };
            // SAFETY: the handle is valid; no overlapped I/O.
            let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if connected {
                let mut message = Vec::new();
                if pipe.take(MAX_MESSAGE).read_to_end(&mut message).is_ok() {
                    on_message(&message);
                }
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
            std::thread::spawn(move || serve(&server, |m| tx.send(m.to_vec()).unwrap()));

            send(&name, b"C:\\one\n").unwrap();
            send(&name, b"").unwrap();
            let timeout = std::time::Duration::from_secs(5);
            assert_eq!(rx.recv_timeout(timeout).unwrap(), b"C:\\one\n");
            assert_eq!(rx.recv_timeout(timeout).unwrap(), b"");
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
