//! Updates from GitHub releases. Graft asks GitHub for the latest release
//! and checks every download against the release's `SHA256SUMS.txt`.
//!
//! - Installed by the setup program (or winget): the new setup runs once
//!   Graft has closed, so "Installed apps" and winget see the new version,
//!   and starts Graft again.
//! - Anywhere else (the zip): `graft-x64.exe` takes the running program's
//!   place and Graft restarts.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::installed;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
pub const REPO_URL: &str = "https://github.com/mdi1984/graft";
const LATEST: &str = "https://api.github.com/repos/mdi1984/graft/releases/latest";
/// Points the check at another "latest release" document, to try the
/// updater against a local server.
const LATEST_OVERRIDE: &str = "GRAFT_UPDATE_URL";
const ASSET: &str = "graft-x64.exe";
const SUMS: &str = "SHA256SUMS.txt";
/// No Graft build comes near this; anything bigger is not a Graft build.
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

/// Where the updater stands, for the About dialog and the title bar.
#[derive(Clone, Debug, Default)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing(Release),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The release page on GitHub, with its notes.
    pub page: String,
    exe: String,
    sums: String,
    /// The setup program; releases before 1.0.0 had none.
    setup: Option<String>,
}

impl Release {
    pub fn is_newer(&self) -> bool {
        newer(&self.version, CURRENT)
    }
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Whether Graft checks on its own at start. Builds for development don't,
/// unless they are pointed at a test server.
pub fn check_on_start() -> bool {
    !cfg!(debug_assertions) || std::env::var_os(LATEST_OVERRIDE).is_some()
}

/// The latest published release.
pub fn check() -> Result<Release> {
    let url = std::env::var(LATEST_OVERRIDE).unwrap_or_else(|_| LATEST.to_owned());
    check_at(&url)
}

fn check_at(url: &str) -> Result<Release> {
    let mut response = agent()
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => anyhow::anyhow!("No release is published yet."),
            ureq::Error::StatusCode(403 | 429) => {
                anyhow::anyhow!("GitHub limits how often Graft may ask. Try again in an hour.")
            }
            e => anyhow::anyhow!("Could not reach GitHub: {e}"),
        })?;
    let text = response
        .body_mut()
        .read_to_string()
        .context("Could not read GitHub's answer")?;
    parse_release(&text)
}

fn parse_release(text: &str) -> Result<Release> {
    let release: GithubRelease =
        serde_json::from_str(text).context("GitHub's answer was not a release")?;
    let Some(version) = release
        .tag_name
        .strip_prefix('v')
        .filter(|v| parse(v).is_some())
    else {
        bail!(
            "The latest release has an unexpected tag: {}",
            release.tag_name
        );
    };
    let asset = |name: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
            .filter(|url| allowed(url))
            .with_context(|| format!("The release has no {name}."))
    };
    Ok(Release {
        version: version.to_owned(),
        page: release.html_url.clone(),
        exe: asset(ASSET)?,
        sums: asset(SUMS)?,
        setup: asset(&setup_name(version)).ok(),
    })
}

fn setup_name(version: &str) -> String {
    format!("Graft-{version}-x64-setup.exe")
}

/// Downloads are only fetched over HTTPS (or from this machine, for tests).
fn allowed(url: &str) -> bool {
    url.starts_with("https://")
        || url.starts_with("http://127.0.0.1:")
        || url.starts_with("http://localhost:")
}

/// What [`install`] left ready for [`restart`].
#[derive(Debug)]
pub enum Installed {
    /// The new program is in place; start it.
    Program(PathBuf),
    /// Run this setup program for the Graft at `exe`.
    Setup { setup: PathBuf, exe: PathBuf },
}

/// Download `release` and check it. A Graft installed by the setup program
/// gets the new setup; any other has its program file replaced.
pub fn install(release: &Release) -> Result<Installed> {
    let exe = std::env::current_exe().context("Could not find Graft's own program file")?;
    if let Some(url) = &release.setup
        && installed::by_setup(installed::KEY, &exe)
    {
        let dir = update_dir();
        let setup = fetch_setup(release, url, &dir)?;
        // Read on the next start, to tell whether the setup finished.
        std::fs::write(dir.join(PENDING), &release.version)
            .with_context(|| format!("Could not write to {}", dir.display()))?;
        return Ok(Installed::Setup { setup, exe });
    }
    install_to(release, &exe)?;
    Ok(Installed::Program(exe))
}

fn install_to(release: &Release, exe: &Path) -> Result<()> {
    let bytes = fetch(release, &release.exe, ASSET)?;
    replace(exe, &bytes)
}

/// Save the setup program into `dir`, once it is checked.
fn fetch_setup(release: &Release, url: &str, dir: &Path) -> Result<PathBuf> {
    let name = setup_name(&release.version);
    let bytes = fetch(release, url, &name)?;
    let path = dir.join(&name);
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&path, bytes))
        .with_context(|| format!("Could not save {}", path.display()))?;
    Ok(path)
}

/// Download `url`, which must match the checksum the release lists for
/// `name`.
fn fetch(release: &Release, url: &str, name: &str) -> Result<Vec<u8>> {
    let agent = agent();
    let download = |url: &str| -> Result<Vec<u8>> {
        agent
            .get(url)
            .call()
            .and_then(|mut r| r.body_mut().with_config().limit(MAX_DOWNLOAD).read_to_vec())
            .with_context(|| format!("Could not download {url}"))
    };
    let sums = String::from_utf8_lossy(&download(&release.sums)?).into_owned();
    let expected =
        expected_hash(&sums, name).with_context(|| format!("{SUMS} does not list {name}"))?;
    let bytes = download(url)?;
    if sha256(&bytes) != expected {
        bail!("The download is damaged (its checksum does not match). Nothing was changed.");
    }
    Ok(bytes)
}

/// Start the new Graft, or the setup that installs it; either waits until
/// this Graft has closed.
pub fn restart(installed: &Installed) -> Result<()> {
    let (mut command, program) = match installed {
        Installed::Program(exe) => {
            let mut command = Command::new(exe);
            command
                .arg(AFTER_UPDATE)
                .arg(std::process::id().to_string());
            (command, exe)
        }
        Installed::Setup { setup, exe } => (
            setup_command(setup, exe, std::process::id(), &setup.with_file_name(LOG)),
            setup,
        ),
    };
    if let Err(e) = command.spawn() {
        if let Installed::Setup { setup, .. } = installed {
            let _ = std::fs::remove_file(setup.with_file_name(PENDING));
        }
        return Err(e).with_context(|| format!("Could not start {}", program.display()));
    }
    Ok(())
}

/// The setup program, run without questions: it shows only its progress,
/// waits for process `pid` to end, logs to `log` and starts `exe` again at
/// the end (see installer/graft.iss).
fn setup_command(setup: &Path, exe: &Path, pid: u32, log: &Path) -> Command {
    let mut command = Command::new(setup);
    for arg in setup_args(exe, pid, log) {
        #[cfg(windows)]
        std::os::windows::process::CommandExt::raw_arg(&mut command, arg);
        #[cfg(not(windows))]
        command.arg(arg);
    }
    command
}

/// Inno Setup reads `/NAME="value"`, with quotes around the value only.
fn setup_args(exe: &Path, pid: u32, log: &Path) -> Vec<String> {
    vec![
        "/SILENT".to_owned(),
        "/SUPPRESSMSGBOXES".to_owned(),
        "/NORESTART".to_owned(),
        format!("/WAITPID={pid}"),
        format!("/LAUNCH=\"{}\"", exe.display()),
        format!("/LOG=\"{}\"", log.display()),
    ]
}

/// Where setup programs are saved before they run.
fn update_dir() -> PathBuf {
    std::env::temp_dir().join("Graft-update")
}

/// In [`update_dir`]: the version a setup is installing, and its log.
const PENDING: &str = "pending";
const LOG: &str = "setup.log";

/// The argument a restarted Graft gets, followed by the old one's process id.
pub const AFTER_UPDATE: &str = "--after-update";

/// Remove what the last update left behind, and correct the version in
/// "Installed apps" if an update by file swap left the old one there. Only
/// the Graft that keeps running calls this. Returns how the last update by
/// setup went, if there was one: the message to show.
pub fn clean_up() -> Option<Result<String, String>> {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_path(&exe));
        installed::record_version(installed::KEY, &exe, CURRENT);
    }
    last_setup(&update_dir(), CURRENT)
}

/// How the setup in `dir` went, now that Graft `current` runs. Removes
/// everything there but the setup's log. A setup that is still finishing
/// stays until the next start.
fn last_setup(dir: &Path, current: &str) -> Option<Result<String, String>> {
    let pending = std::fs::read_to_string(dir.join(PENDING)).ok();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten().filter(|e| e.file_name() != LOG) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let version = pending?.trim().to_owned();
    Some(if version == current {
        Ok(format!("Updated to Graft {current}."))
    } else {
        Err(format!(
            "The update to Graft {version} did not finish; this is still Graft {current}. \
             The setup's log: {}",
            dir.join(LOG).display()
        ))
    })
}

fn agent() -> ureq::Agent {
    // Windows' own TLS and certificate store, so company proxies that
    // inspect TLS keep working.
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(format!("Graft/{CURRENT}"))
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into()
}

fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_owned();
    name.push(".old");
    exe.with_file_name(name)
}

/// Windows lets a running program be renamed but not overwritten: the old
/// one moves aside (removed on the next start) and the new one takes its name.
fn replace(exe: &Path, bytes: &[u8]) -> Result<()> {
    let dir = exe.parent().context("Graft's program file has no folder")?;
    let mut name = exe.file_name().unwrap_or_default().to_owned();
    name.push(".new");
    let new = exe.with_file_name(name);
    let old = old_path(exe);
    std::fs::write(&new, bytes)
        .with_context(|| format!("Graft cannot write to {}", dir.display()))?;
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(e).with_context(|| format!("Could not replace {}", exe.display()));
    }
    if let Err(e) = std::fs::rename(&new, exe) {
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&new);
        return Err(e).with_context(|| format!("Could not replace {}", exe.display()));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The hash for `name` in a `sha256sum`-style list ("<hash>  <name>").
fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        (file.trim_start().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
    })
}

/// `major.minor.patch`; anything else (pre-releases included) is not a version.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

fn newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("1.0.1", "1.0.0"));
        assert!(newer("1.10.0", "1.9.3"));
        assert!(!newer("1.0.0", "1.0.0"));
        assert!(!newer("0.9.9", "1.0.0"));
        assert!(!newer("2.0.0-beta", "1.0.0"));
        assert!(!newer("2.0", "1.0.0"));
    }

    #[test]
    fn a_github_release_is_read() {
        let json = r#"{
            "tag_name": "v1.2.3",
            "html_url": "https://github.com/mdi1984/graft/releases/tag/v1.2.3",
            "assets": [
                {"name": "Graft-1.2.3-x64.zip", "browser_download_url": "https://example.com/z"},
                {"name": "Graft-1.2.3-x64-setup.exe", "browser_download_url": "https://example.com/s"},
                {"name": "graft-x64.exe", "browser_download_url": "https://example.com/exe"},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.com/sums"}
            ]
        }"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.version, "1.2.3");
        assert_eq!(release.exe, "https://example.com/exe");
        assert_eq!(release.sums, "https://example.com/sums");
        assert_eq!(release.setup.as_deref(), Some("https://example.com/s"));
        // Without a setup program, updates swap the program file.
        let json = json.replace("Graft-1.2.3-x64-setup.exe", "other.exe");
        assert_eq!(parse_release(&json).unwrap().setup, None);
    }

    #[test]
    fn plain_http_downloads_are_refused() {
        let json = r#"{
            "tag_name": "v1.2.3",
            "html_url": "https://github.com/mdi1984/graft/releases/tag/v1.2.3",
            "assets": [
                {"name": "graft-x64.exe", "browser_download_url": "http://example.com/exe"},
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.com/sums"}
            ]
        }"#;
        assert!(parse_release(json).is_err());
    }

    #[test]
    fn the_checksum_list_is_read() {
        let sums = "AB12  Graft-1.0.0-x64.zip\r\ncd34  graft-x64.exe\r\n";
        assert_eq!(
            expected_hash(sums, "graft-x64.exe").as_deref(),
            Some("cd34")
        );
        assert_eq!(expected_hash(sums, "other.exe"), None);
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Answers each request with the file of that name; unknown paths get 404.
    fn serve(files: Vec<(&'static str, Vec<u8>)>) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!(
            "http://127.0.0.1:{}/",
            listener.local_addr().unwrap().port()
        );
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 2 {
                    line.clear();
                }
                let path = request
                    .split(' ')
                    .nth(1)
                    .unwrap_or("")
                    .trim_start_matches('/');
                match files.iter().find(|(name, _)| *name == path) {
                    Some((_, body)) => {
                        let head = format!(
                            "HTTP/1.1 200 OK
Content-Length: {}
Connection: close

",
                            body.len()
                        );
                        stream.write_all(head.as_bytes()).unwrap();
                        stream.write_all(body).unwrap();
                    }
                    None => stream
                        .write_all(
                            b"HTTP/1.1 404 Not Found
Content-Length: 0
Connection: close

",
                        )
                        .unwrap(),
                }
            }
        });
        base
    }

    fn release_json(base: &str, version: &str) -> Vec<u8> {
        format!(
            r#"{{"tag_name": "v{version}", "html_url": "{base}page", "assets": [
                {{"name": "graft-x64.exe", "browser_download_url": "{base}graft-x64.exe"}},
                {{"name": "Graft-{version}-x64-setup.exe",
                  "browser_download_url": "{base}Graft-{version}-x64-setup.exe"}},
                {{"name": "SHA256SUMS.txt", "browser_download_url": "{base}SHA256SUMS.txt"}}
            ]}}"#
        )
        .into_bytes()
    }

    #[test]
    fn an_update_is_downloaded_checked_and_swapped_in() {
        let new = b"the new graft".to_vec();
        let sums = format!(
            "{}  graft-x64.exe
",
            sha256(&new)
        )
        .into_bytes();
        // The release document names the server's own address, so it is
        // served from a second server that knows the first one's address.
        let files = serve(vec![
            ("graft-x64.exe", new.clone()),
            ("SHA256SUMS.txt", sums),
        ]);
        let api = serve(vec![("latest", release_json(&files, "99.0.0"))]);

        let release = check_at(&format!("{api}latest")).unwrap();
        assert_eq!(release.version, "99.0.0");
        assert!(release.is_newer());

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("graft.exe");
        std::fs::write(&exe, b"the old graft").unwrap();
        install_to(&release, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), new);
    }

    #[test]
    fn a_damaged_download_changes_nothing() {
        let sums = format!(
            "{}  graft-x64.exe
",
            sha256(b"what was published")
        )
        .into_bytes();
        let files = serve(vec![
            ("graft-x64.exe", b"something else".to_vec()),
            ("SHA256SUMS.txt", sums),
        ]);
        let api = serve(vec![("latest", release_json(&files, "99.0.0"))]);
        let release = check_at(&format!("{api}latest")).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("graft.exe");
        std::fs::write(&exe, b"the old graft").unwrap();
        let error = install_to(&release, &exe).unwrap_err();
        assert!(format!("{error:#}").contains("checksum"), "{error:#}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"the old graft");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn the_setup_program_is_downloaded_and_checked() {
        let setup = b"the new setup".to_vec();
        let sums = format!(
            "{}  Graft-99.0.0-x64-setup.exe\n{}  graft-x64.exe\n",
            sha256(&setup),
            sha256(b"the new graft")
        )
        .into_bytes();
        let files = serve(vec![
            ("Graft-99.0.0-x64-setup.exe", setup.clone()),
            ("SHA256SUMS.txt", sums),
        ]);
        let api = serve(vec![("latest", release_json(&files, "99.0.0"))]);
        let release = check_at(&format!("{api}latest")).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let saved = fetch_setup(&release, release.setup.as_deref().unwrap(), dir.path()).unwrap();
        assert_eq!(saved, dir.path().join("Graft-99.0.0-x64-setup.exe"));
        assert_eq!(std::fs::read(&saved).unwrap(), setup);
    }

    #[test]
    fn a_damaged_setup_program_is_not_saved() {
        let sums = format!(
            "{}  Graft-99.0.0-x64-setup.exe\n",
            sha256(b"what was published")
        )
        .into_bytes();
        let files = serve(vec![
            ("Graft-99.0.0-x64-setup.exe", b"something else".to_vec()),
            ("SHA256SUMS.txt", sums),
        ]);
        let api = serve(vec![("latest", release_json(&files, "99.0.0"))]);
        let release = check_at(&format!("{api}latest")).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let error =
            fetch_setup(&release, release.setup.as_deref().unwrap(), dir.path()).unwrap_err();
        assert!(format!("{error:#}").contains("checksum"), "{error:#}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_setup_is_told_what_to_wait_for_and_start() {
        let args = setup_args(
            Path::new(r"C:\Users\A B\Programs\Graft\graft.exe"),
            42,
            Path::new(r"C:\Temp\Graft-update\setup.log"),
        );
        assert_eq!(
            args,
            [
                "/SILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/WAITPID=42",
                r#"/LAUNCH="C:\Users\A B\Programs\Graft\graft.exe""#,
                r#"/LOG="C:\Temp\Graft-update\setup.log""#,
            ]
        );
    }

    /// One real update through the setup program: Graft must be installed
    /// by its setup, and `GRAFT_E2E_SETUP` names a newer setup whose
    /// version is `GRAFT_E2E_VERSION`. It changes the installed Graft, so it
    /// only runs when asked; CI's package job does (see ci.yml).
    #[test]
    #[ignore = "installs Graft; run by CI"]
    fn a_setup_install_is_updated_by_its_setup() {
        use std::time::{Duration, Instant};
        let setup = PathBuf::from(std::env::var("GRAFT_E2E_SETUP").unwrap());
        let version = std::env::var("GRAFT_E2E_VERSION").unwrap();
        let location = installed::location(installed::KEY).expect("Graft is not installed");
        let exe = Path::new(&location).join("graft.exe");
        assert!(installed::by_setup(installed::KEY, &exe));
        assert_ne!(installed::version(installed::KEY), Some(version.clone()));

        // Stands in for the running Graft (about 10 seconds); the setup
        // must not replace anything before it ends.
        let mut running = Command::new("ping")
            .args(["-n", "11", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("setup.log");
        let started = Instant::now();
        let status = setup_command(&setup, &exe, running.id(), &log)
            .status()
            .unwrap();
        let log = String::from_utf8_lossy(&std::fs::read(&log).unwrap()).into_owned();
        assert!(status.success(), "the setup failed: {status}\n{log}");
        assert!(running.try_wait().unwrap().is_some(), "{log}");
        assert!(started.elapsed() >= Duration::from_secs(9), "{log}");
        assert_eq!(installed::version(installed::KEY), Some(version), "{log}");
        assert!(log.contains("Started Graft again"), "{log}");
        let _ = Command::new("taskkill")
            .args(["/IM", "graft.exe", "/F"])
            .status();
    }

    #[test]
    fn the_next_start_tells_how_the_setup_went() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(last_setup(dir.path(), "1.0.2"), None);

        let leftovers = |version: &str| {
            std::fs::write(dir.path().join(PENDING), version).unwrap();
            std::fs::write(dir.path().join("Graft-1.0.2-x64-setup.exe"), b"setup").unwrap();
            std::fs::write(dir.path().join(LOG), b"log").unwrap();
        };
        leftovers("1.0.2");
        assert_eq!(
            last_setup(dir.path(), "1.0.2"),
            Some(Ok("Updated to Graft 1.0.2.".to_owned()))
        );
        leftovers("1.0.2");
        let failed = last_setup(dir.path(), "1.0.1").unwrap().unwrap_err();
        assert!(failed.contains("did not finish"), "{failed}");
        assert!(failed.contains(&dir.path().join(LOG).display().to_string()));
        // Only the log stays, for that message; next time there is nothing to say.
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, [LOG]);
        assert_eq!(last_setup(dir.path(), "1.0.1"), None);
    }

    #[test]
    fn no_release_yet_reads_plainly() {
        let api = serve(vec![]);
        let error = check_at(&format!("{api}latest")).unwrap_err();
        assert_eq!(error.to_string(), "No release is published yet.");
    }

    #[test]
    fn the_program_file_is_swapped() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("graft.exe");
        std::fs::write(&exe, b"old").unwrap();
        replace(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"old");
        // A second update replaces the leftover from the first.
        replace(&exe, b"newer").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"newer");
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"new");
    }
}
