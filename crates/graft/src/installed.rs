//! The setup program's record of Graft, shown in "Installed apps" and read
//! by winget: where Graft was installed and which version is there.

use std::path::Path;

/// The record Inno Setup keeps for Graft's AppId (see installer/graft.iss).
pub const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{F50FBE64-D67E-4FC3-A67B-CF64F3A8535F}_is1";

/// The folder the setup program installed Graft to, if it did.
pub fn location(key: &str) -> Option<String> {
    reg::get(key, "InstallLocation")
}

/// The version the record names.
pub fn version(key: &str) -> Option<String> {
    reg::get(key, "DisplayVersion")
}

/// Was `exe` installed by the setup program? Only when the record names
/// `exe`'s own folder: a copy unpacked from the zip elsewhere is not.
pub fn by_setup(key: &str, exe: &Path) -> bool {
    let (Some(location), Some(folder)) = (location(key), exe.parent()) else {
        return false;
    };
    match (
        std::fs::canonicalize(location.trim_end_matches('\\')),
        std::fs::canonicalize(folder),
    ) {
        (Ok(a), Ok(b)) => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
        _ => false,
    }
}

/// Make the record name `version` when it is about `exe`. Updates that only
/// swapped the program file (Graft 1.0.1 and older did) left the old
/// version there. Returns whether it changed anything.
pub fn record_version(key: &str, exe: &Path, version: &str) -> bool {
    if !by_setup(key, exe) || self::version(key).as_deref() == Some(version) {
        return false;
    }
    match reg::set(key, "DisplayVersion", version) {
        Ok(()) => true,
        Err(e) => {
            tracing::info!("could not record version {version}: {e}");
            false
        }
    }
}

/// Strings under HKEY_CURRENT_USER.
#[cfg(windows)]
mod reg {
    use std::ptr::null_mut;

    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    pub fn get(key: &str, name: &str) -> Option<String> {
        let (key, name) = (wide(key), wide(name));
        let mut size = 0u32;
        // SAFETY: the strings are NUL-terminated and outlive the calls; the
        // buffer is as large as the size passed with it.
        unsafe {
            let status = RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                null_mut(),
                &mut size,
            );
            if status != 0 {
                return None;
            }
            let mut buffer = vec![0u16; size as usize / 2 + 1];
            size = (buffer.len() * 2) as u32;
            let status = RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            );
            if status != 0 {
                return None;
            }
            let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            Some(String::from_utf16_lossy(&buffer[..len]))
        }
    }

    /// Creates the key if it is missing.
    pub fn set(key: &str, name: &str, value: &str) -> std::io::Result<()> {
        let (key, name, value) = (wide(key), wide(name), wide(value));
        // SAFETY: as in `get`; the size covers the value and its NUL.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        match status {
            0 => Ok(()),
            e => Err(std::io::Error::from_raw_os_error(e as i32)),
        }
    }

    #[cfg(test)]
    pub fn delete(key: &str) {
        use windows_sys::Win32::System::Registry::RegDeleteTreeW;
        let key = wide(key);
        // SAFETY: as in `get`.
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
    }
}

#[cfg(not(windows))]
mod reg {
    pub fn get(_key: &str, _name: &str) -> Option<String> {
        None
    }

    pub fn set(_key: &str, _name: &str, _value: &str) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// A record of its own under HKEY_CURRENT_USER, removed afterwards.
    struct Record(String);

    impl Record {
        fn new(name: &str) -> Self {
            let key = format!(r"Software\Graft-tests\{name}-{}", std::process::id());
            reg::delete(&key);
            Self(key)
        }
    }

    impl Drop for Record {
        fn drop(&mut self) {
            reg::delete(&self.0);
        }
    }

    #[test]
    fn a_setup_install_is_told_from_a_copy_elsewhere() {
        let record = Record::new("location");
        let installed = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let exe = installed.path().join("graft.exe");
        assert!(!by_setup(&record.0, &exe), "no record yet");

        // Inno Setup writes the folder with a trailing backslash.
        let location = format!(r"{}\", installed.path().display());
        reg::set(&record.0, "InstallLocation", &location).unwrap();
        assert!(by_setup(&record.0, &exe));
        let upper = installed.path().display().to_string().to_uppercase();
        reg::set(&record.0, "InstallLocation", &upper).unwrap();
        assert!(by_setup(&record.0, &exe), "case does not matter");
        assert!(!by_setup(&record.0, &elsewhere.path().join("graft.exe")));
    }

    #[test]
    fn the_recorded_version_follows_the_program() {
        let record = Record::new("version");
        let installed = tempfile::tempdir().unwrap();
        let exe = installed.path().join("graft.exe");
        reg::set(&record.0, "DisplayVersion", "1.0.1").unwrap();
        assert!(!record_version(&record.0, &exe, "1.0.2"), "not this copy");
        assert_eq!(version(&record.0).as_deref(), Some("1.0.1"));

        let location = installed.path().display().to_string();
        reg::set(&record.0, "InstallLocation", &location).unwrap();
        assert!(record_version(&record.0, &exe, "1.0.2"));
        assert_eq!(version(&record.0).as_deref(), Some("1.0.2"));
        assert!(!record_version(&record.0, &exe, "1.0.2"), "already right");
    }
}
