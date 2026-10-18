//! Installing a downloaded build, which needs the app gone: it hands over to
//! a process that outlives it, then the app quits.
//!
//! - Windows, installed by the Inno Setup installer: the new installer,
//!   silent. It closes what's left of the app, replaces the files, and starts
//!   the new version (`/relaunch=1`, see beardictate.iss).
//! - Linux, installed by install.sh under ~/.local/lib/beardictate: a shell
//!   waits for the app to exit, runs the new tarball's install.sh, and starts
//!   the app again.
//!
//! Anything else (a portable zip, a development build) only hears that a
//! new version is out.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// How this copy of the app was installed.
#[derive(Debug, Clone, PartialEq)]
pub enum Install {
    /// Windows, by the installer (its uninstaller is next to the app).
    Inno,
    /// Linux, by install.sh: the app is `~/.local/lib/beardictate/beardictate`.
    LinuxUser { exe: PathBuf },
    /// A portable copy, a development build, or anything else.
    Unmanaged,
}

impl Install {
    pub fn detect() -> Install {
        let Ok(exe) = std::env::current_exe().and_then(|e| e.canonicalize()) else {
            return Install::Unmanaged;
        };
        if cfg!(windows) {
            if exe
                .parent()
                .is_some_and(|d| d.join("unins000.exe").is_file())
            {
                return Install::Inno;
            }
            return Install::Unmanaged;
        }
        let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) else {
            return Install::Unmanaged;
        };
        let installed = home.join(".local/lib/beardictate/beardictate");
        if installed.canonicalize().is_ok_and(|i| i == exe) {
            return Install::LinuxUser { exe: installed };
        }
        Install::Unmanaged
    }

    pub fn can_update(&self) -> bool {
        *self != Install::Unmanaged
    }
}

/// Starts installing `file` (a verified build) in a process of its own,
/// logging to `log`. The caller quits straight after.
pub fn start(install: &Install, file: &Path, work: &Path, log: &Path) -> Result<()> {
    match install {
        Install::Inno => start_installer(file, log),
        Install::LinuxUser { exe } => start_linux(file, work, log, exe),
        Install::Unmanaged => {
            anyhow::bail!("this copy of beardictate wasn't installed by its installer")
        }
    }
}

#[cfg(windows)]
fn start_installer(file: &Path, log: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    Command::new(file)
        .args([
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/CLOSEAPPLICATIONS",
            "/relaunch=1",
        ])
        .arg(format!("/LOG={}", log.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .with_context(|| format!("starting {}", file.display()))?;
    Ok(())
}

#[cfg(not(windows))]
fn start_installer(_file: &Path, _log: &Path) -> Result<()> {
    anyhow::bail!("the Windows installer runs only on Windows")
}

#[cfg(not(windows))]
fn start_linux(file: &Path, work: &Path, log: &Path, exe: &Path) -> Result<()> {
    let unpacked = work.join("unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked)
        .with_context(|| format!("creating {}", unpacked.display()))?;
    let tarball =
        std::fs::File::open(file).with_context(|| format!("opening {}", file.display()))?;
    tar::Archive::new(flate2::read::GzDecoder::new(tarball))
        .unpack(&unpacked)
        .with_context(|| format!("unpacking {}", file.display()))?;
    let script = find_install_sh(&unpacked).context("the update has no install.sh")?;
    // Wait for this process to go, install, and start the app again (the
    // new one, or this one if the install failed: it then says so).
    const SCRIPT: &str = r#"
while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
{ echo "installing $(date)"; "$2"; echo "install.sh: $?"; } >"$3" 2>&1
rm -rf "$4"
exec "$5"
"#;
    Command::new("/bin/sh")
        .arg("-c")
        .arg(SCRIPT)
        .arg("beardictate-update")
        .arg(std::process::id().to_string())
        .arg(&script)
        .arg(log)
        .arg(&unpacked)
        .arg(exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting the installer shell")?;
    Ok(())
}

#[cfg(windows)]
fn start_linux(_file: &Path, _work: &Path, _log: &Path, _exe: &Path) -> Result<()> {
    anyhow::bail!("install.sh runs only on Linux")
}

/// `install.sh` at the top of the tarball's one folder.
#[cfg(not(windows))]
fn find_install_sh(dir: &Path) -> Option<PathBuf> {
    let direct = dir.join("install.sh");
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path().join("install.sh"))
        .find(|p| p.is_file())
}
