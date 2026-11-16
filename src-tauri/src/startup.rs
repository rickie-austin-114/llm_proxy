use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use proxy_service::error::AppError;
use serde::{Deserialize, Serialize};

const MARKER: &str = "worker-startup.json";
#[cfg(target_os = "macos")]
const AGENT: &str = "app.llmproxy.worker.plist";

#[derive(Serialize, Deserialize)]
struct Registration {
    fingerprint: String,
    reset: bool,
}

pub fn worker_path() -> Result<PathBuf, AppError> {
    let executable = std::env::current_exe().map_err(|_| error())?;
    let directory = executable.parent().ok_or_else(error)?;
    let name = if cfg!(windows) {
        "llm-proxy-worker.exe"
    } else {
        "llm-proxy-worker"
    };
    let worker = directory.join(name);
    if !worker.is_file() {
        return Err(AppError::new(
            "worker_missing",
            "The background worker is missing. Reinstall the complete app.",
        ));
    }
    Ok(worker)
}

fn save(directory: &Path, registration: &Registration) -> Result<(), AppError> {
    fs::create_dir_all(directory).map_err(|_| error())?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(|_| error())?;
    serde_json::to_writer(&mut temporary, registration).map_err(|_| error())?;
    temporary.flush().map_err(|_| error())?;
    temporary.as_file().sync_all().map_err(|_| error())?;
    temporary
        .persist(directory.join(MARKER))
        .map_err(|_| error())?;
    Ok(())
}

fn load(directory: &Path) -> Result<Option<Registration>, AppError> {
    match fs::read(directory.join(MARKER)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| error()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(error()),
    }
}

pub fn needs_worker_handoff(directory: &Path, new_setup: bool) -> Result<bool, AppError> {
    if cfg!(debug_assertions) || !new_setup {
        return Ok(false);
    }
    Ok(load(directory)?.is_none_or(|registration| registration.reset))
}

pub fn initialize(directory: &Path, worker: &Path, new_setup: bool) -> Result<String, AppError> {
    // Development sessions never change login items, scheduled tasks, or legacy registration.
    if cfg!(debug_assertions) {
        return Ok("development".into());
    }
    #[cfg(target_os = "macos")]
    if !worker
        .parent()
        .and_then(Path::parent)
        .is_some_and(|contents| contents.join("Library/LaunchAgents").join(AGENT).is_file())
    {
        return Ok("installation_required".into());
    }
    let fingerprint = proxy_service::binary_fingerprint(worker)?;
    let previous = load(directory)?;
    if previous
        .as_ref()
        .is_some_and(|registration| registration.reset)
        && !new_setup
    {
        return Ok("disabled".into());
    }
    let state = platform::status()?;
    if previous.is_none()
        || previous
            .as_ref()
            .is_some_and(|registration| registration.reset && new_setup)
    {
        platform::register(worker)?;
        save(
            directory,
            &Registration {
                fingerprint,
                reset: false,
            },
        )?;
    } else if previous
        .as_ref()
        .is_some_and(|registration| registration.fingerprint != fingerprint)
    {
        // Respect OS-disabled startup across app updates. Enabled agents are re-registered
        // so ServiceManagement records the new executable signature.
        if state == "enabled" {
            platform::unregister()?;
            platform::register(worker)?;
            save(
                directory,
                &Registration {
                    fingerprint,
                    reset: false,
                },
            )?;
        }
        // Retain the old fingerprint while OS-disabled. If approval is restored later,
        // the next launch still knows it must refresh the registered executable identity.
    }
    platform::remove_legacy()?;
    platform::status()
}

pub fn disable(directory: &Path) -> Result<(), AppError> {
    if !cfg!(debug_assertions) {
        platform::unregister()?;
        platform::remove_legacy()?;
    }
    save(
        directory,
        &Registration {
            fingerprint: String::new(),
            reset: true,
        },
    )
}

pub fn launch(worker: &Path, state: &str) -> Result<(), AppError> {
    if state == "enabled" && !cfg!(debug_assertions) {
        return platform::launch();
    }
    let mut command = std::process::Command::new(worker);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|_| error())?;
    // The worker intentionally outlives the GUI. Reap a failed manual launch without keeping
    // the GUI runtime alive or terminating the worker when its parent exits.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn error() -> AppError {
    AppError::new("startup_registration_failed", "Automatic background startup could not be configured. Install the complete app and check your OS login/background-app permissions. The proxy can still run while signed in.")
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use objc2::{rc::autoreleasepool, rc::Retained};
    use objc2_foundation::{NSError, NSString};
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    fn agent() -> Retained<SMAppService> {
        // SAFETY: this constant names the LaunchAgent plist shipped in our signed app bundle.
        unsafe { SMAppService::agentServiceWithPlistName(&NSString::from_str(AGENT)) }
    }

    pub fn status() -> Result<String, AppError> {
        autoreleasepool(|_| {
            // SAFETY: the retained service is live; status has no preconditions.
            let status = unsafe { agent().status() };
            Ok(if status == SMAppServiceStatus::Enabled {
                "enabled"
            } else if status == SMAppServiceStatus::RequiresApproval {
                "approval_required"
            } else {
                "disabled"
            }
            .into())
        })
    }

    pub fn register(_worker: &Path) -> Result<(), AppError> {
        autoreleasepool(|_| {
            if status()? == "enabled" || status()? == "approval_required" {
                return Ok(());
            }
            // SAFETY: registration is for our bundled per-user agent; OS consent remains enforced.
            unsafe { agent().registerAndReturnError() }.map_err(|_| error())
        })
    }

    pub fn unregister() -> Result<(), AppError> {
        autoreleasepool(|_| {
            if status()? == "disabled" {
                return Ok(());
            }
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            let completion = block2::RcBlock::new(move |native_error: *mut NSError| {
                let _ = sender.send(native_error.is_null());
            });
            // SAFETY: this only unregisters our bundled service. ServiceManagement copies
            // the completion block and invokes it once on a dispatch queue. Captured state
            // is thread-safe and never dereferences the borrowed NSError pointer.
            unsafe { agent().unregisterWithCompletionHandler(&completion) };
            // The synchronous unregister API may return before the old process is reaped.
            // Wait off the networking runtime for completion before any replacement register.
            match receiver.recv_timeout(std::time::Duration::from_secs(15)) {
                Ok(true) => Ok(()),
                _ => Err(error()),
            }
        })
    }

    pub fn remove_legacy() -> Result<(), AppError> {
        let path = proxy_service::config_dir()?
            .parent()
            .and_then(Path::parent)
            .ok_or_else(error)?
            .join("LaunchAgents/llm_proxy.plist");
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(error()),
        };
        if !contents.contains("/Contents/MacOS/llm-proxy") || !contents.contains("--background") {
            return Err(error());
        }
        // bootout is intentionally limited to the known legacy per-user job, never the new agent.
        let _ = std::process::Command::new("/bin/launchctl")
            .arg("bootout")
            .arg(format!("gui/{}", proxy_service::user_id()?))
            .arg(&path)
            .output();
        fs::remove_file(path).map_err(|_| error())
    }

    pub fn launch() -> Result<(), AppError> {
        let output = std::process::Command::new("/bin/launchctl")
            .arg("kickstart")
            .arg(format!(
                "gui/{}/app.llmproxy.worker",
                proxy_service::user_id()?
            ))
            .output()
            .map_err(|_| error())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(error())
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Output};

    fn command() -> Command {
        let mut command = Command::new("schtasks.exe");
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        command
    }

    fn task_name() -> Result<String, AppError> {
        Ok(format!("llm_proxy-{}", proxy_service::user_id()?))
    }

    fn query() -> Result<Output, AppError> {
        command()
            .args(["/Query", "/TN", &task_name()?, "/XML"])
            .output()
            .map_err(|_| error())
    }

    pub fn status() -> Result<String, AppError> {
        let output = query()?;
        if !output.status.success() {
            return Ok("disabled".into());
        }
        let utf16 = output.stdout.starts_with(&[0xff, 0xfe]) || output.stdout.get(1) == Some(&0);
        let offset = if output.stdout.starts_with(&[0xff, 0xfe]) {
            2
        } else {
            0
        };
        let text = if utf16 {
            String::from_utf16_lossy(
                &output.stdout[offset..]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair))
                    .collect::<Vec<_>>(),
            )
        } else {
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        Ok(if text.contains("<Enabled>false</Enabled>") {
            "disabled"
        } else {
            "enabled"
        }
        .into())
    }

    pub fn register(worker: &Path) -> Result<(), AppError> {
        let sid = proxy_service::user_id()?;
        let executable = worker.to_str().ok_or_else(error)?;
        let xml = super::task_xml(&sid, executable);
        let mut file = tempfile::NamedTempFile::new().map_err(|_| error())?;
        file.write_all(&[0xff, 0xfe]).map_err(|_| error())?;
        for unit in xml.encode_utf16() {
            file.write_all(&unit.to_le_bytes()).map_err(|_| error())?;
        }
        file.flush().map_err(|_| error())?;
        let output = command()
            .args(["/Create", "/TN", &task_name()?, "/XML"])
            .arg(file.path())
            .arg("/F")
            .output()
            .map_err(|_| error())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(error())
        }
    }

    pub fn unregister() -> Result<(), AppError> {
        if !query()?.status.success() {
            return Ok(());
        }
        let output = command()
            .args(["/Delete", "/TN", &task_name()?, "/F"])
            .output()
            .map_err(|_| error())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(error())
        }
    }

    pub fn remove_legacy() -> Result<(), AppError> {
        for name in ["llm_proxy", "llm-proxy"] {
            let mut command = Command::new("reg.exe");
            command.creation_flags(0x08000000).args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                name,
                "/f",
            ]);
            command.output().map_err(|_| error())?;
        }
        Ok(())
    }

    pub fn launch() -> Result<(), AppError> {
        let output = command()
            .args(["/Run", "/TN", &task_name()?])
            .output()
            .map_err(|_| error())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(error())
        }
    }
}

#[cfg(any(windows, test))]
fn task_xml(sid: &str, executable: &str) -> String {
    let escaped = executable
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Runs the llm_proxy background worker after sign-in.</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{sid}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="User"><UserId>{sid}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><AllowHardTerminate>true</AllowHardTerminate><StartWhenAvailable>true</StartWhenAvailable><Enabled>true</Enabled><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure></Settings>
  <Actions Context="User"><Exec><Command>{escaped}</Command><Arguments>--managed</Arguments></Exec></Actions>
</Task>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logon_task_uses_interactive_user_without_passwords_and_escapes_installed_paths() {
        let xml = task_xml("S-1-5-21-123", r"C:\Users\A & B\llm-proxy-worker.exe");
        assert!(xml.contains("A &amp; B"));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(!xml.contains("<Password>"));
    }

    #[test]
    fn reset_marker_survives_relaunch_and_only_new_setup_requests_enrollment() {
        let directory = tempfile::tempdir().expect("isolated registration");
        save(
            directory.path(),
            &Registration {
                fingerprint: String::new(),
                reset: true,
            },
        )
        .expect("reset marker");
        assert!(!needs_worker_handoff(directory.path(), false).expect("relaunch preserves reset"));
        if !cfg!(debug_assertions) {
            assert!(needs_worker_handoff(directory.path(), true).expect("explicit setup enrolls"));
        }
        let saved = load(directory.path())
            .expect("read marker")
            .expect("saved marker");
        assert!(saved.reset);
    }

    #[test]
    fn existing_registration_does_not_restart_a_running_worker_on_provider_save() {
        let directory = tempfile::tempdir().expect("isolated registration");
        save(
            directory.path(),
            &Registration {
                fingerprint: "fake-binary-fingerprint".into(),
                reset: false,
            },
        )
        .expect("registration marker");
        assert!(!needs_worker_handoff(directory.path(), true).expect("normal settings save"));
    }
}
