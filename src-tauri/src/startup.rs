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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Inspect,
    Enroll,
    Repair,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServiceState {
    Enabled,
    #[cfg_attr(
        all(windows, not(test)),
        allow(dead_code, reason = "Only macOS reports pending approval.")
    )]
    ApprovalRequired,
    Disabled,
    #[cfg_attr(
        all(windows, not(test)),
        allow(
            dead_code,
            reason = "Only macOS reports a distinct missing-service status."
        )
    )]
    Missing,
}

impl ServiceState {
    fn registered(self) -> bool {
        matches!(self, Self::Enabled | Self::ApprovalRequired)
    }

    fn display(self) -> Result<String, AppError> {
        match self {
            Self::Enabled => Ok("enabled".into()),
            Self::ApprovalRequired => Ok("approval_required".into()),
            Self::Disabled => Ok("disabled".into()),
            Self::Missing => Err(AppError::new(
                "startup_service_not_found",
                "macOS could not locate the background service. Select Check background startup to retry registration, or finish onboarding if setup is incomplete. If it still fails, share this error message with the app publisher.",
            )),
        }
    }
}

// Keep policy and persistence testable without touching the user's OS registration.
trait StartupService {
    fn status(&self) -> Result<ServiceState, AppError>;
    fn register(&self, worker: &Path) -> Result<(), AppError>;
    fn unregister(&self) -> Result<(), AppError>;
    fn remove_legacy(&self) -> Result<(), AppError>;
}

struct NativeService;

impl StartupService for NativeService {
    fn status(&self) -> Result<ServiceState, AppError> {
        platform::status()
    }
    fn register(&self, worker: &Path) -> Result<(), AppError> {
        platform::register(worker)
    }
    fn unregister(&self) -> Result<(), AppError> {
        platform::unregister()
    }
    fn remove_legacy(&self) -> Result<(), AppError> {
        platform::remove_legacy()
    }
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

pub fn needs_worker_handoff(directory: &Path, intent: Intent) -> Result<bool, AppError> {
    if cfg!(debug_assertions) || intent != Intent::Enroll {
        return Ok(false);
    }
    Ok(load(directory)?.is_none_or(|registration| registration.reset))
}

pub fn initialize(directory: &Path, worker: &Path, intent: Intent) -> Result<String, AppError> {
    if !proxy_service::background_enabled(directory)? {
        return Ok("foreground".into());
    }
    // Development sessions never change login items, scheduled tasks, or legacy registration.
    if cfg!(debug_assertions) {
        return Ok("development".into());
    }
    initialize_with_service(directory, worker, intent, &NativeService)
}

fn initialize_with_service(
    directory: &Path,
    worker: &Path,
    intent: Intent,
    service: &impl StartupService,
) -> Result<String, AppError> {
    if !proxy_service::background_enabled(directory)? {
        return Ok("foreground".into());
    }
    let previous = load(directory)?;
    let setup_pending = intent != Intent::Enroll && proxy_service::onboarding_pending(directory)?;
    // Inspect an already enrolled approval request, but never enroll/repair early.
    if setup_pending
        && previous
            .as_ref()
            .is_none_or(|registration| registration.reset)
    {
        return Ok("setup_required".into());
    }
    if previous
        .as_ref()
        .is_some_and(|registration| registration.reset)
        && intent != Intent::Enroll
    {
        return Ok("disabled".into());
    }
    #[cfg(target_os = "macos")]
    if !validate_bundle(worker)? {
        return Ok("installation_required".into());
    }
    let fingerprint = proxy_service::binary_fingerprint(worker)?;
    let state = service.status()?;
    let first_enrollment = previous.is_none()
        || previous
            .as_ref()
            .is_some_and(|registration| registration.reset && intent == Intent::Enroll);
    let repair_missing =
        !setup_pending && state == ServiceState::Missing && intent != Intent::Inspect;
    let refresh_enabled = !setup_pending
        && state == ServiceState::Enabled
        && previous
            .as_ref()
            .is_some_and(|registration| registration.fingerprint != fingerprint);
    let register = repair_missing
        || refresh_enabled
        || (!setup_pending && first_enrollment && state == ServiceState::Disabled);
    let state = if register {
        if refresh_enabled {
            service.unregister()?;
        }
        let registration = service.register(worker);
        let observed = service.status();
        // A native error can still leave approval pending. Otherwise retain that error,
        // including its sanitized code, rather than hiding it behind a discovery failure.
        match observed {
            Ok(state) if state.registered() => state,
            other => {
                registration?;
                let state = other?;
                state.display()?;
                return Err(AppError::new("startup_registration_incomplete", "Background startup is still disabled after registration. Check OS background-app permissions, then select Check background startup to retry."));
            }
        }
    } else {
        // Preserve OS opt-outs and the old fingerprint until approval is restored.
        state
    };
    if state.registered() {
        if register || first_enrollment {
            save(
                directory,
                &Registration {
                    fingerprint,
                    reset: false,
                },
            )?;
        }
        service.remove_legacy()?;
    }
    state.display()
}

#[cfg(target_os = "macos")]
fn validate_bundle(worker: &Path) -> Result<bool, AppError> {
    use std::os::unix::fs::PermissionsExt;

    let Some(contents) = worker.parent().and_then(Path::parent) else {
        return Ok(false);
    };
    let Some(bundle) = contents.parent() else {
        return Ok(false);
    };
    if contents.file_name().is_none_or(|name| name != "Contents")
        || bundle
            .extension()
            .is_none_or(|extension| extension != "app")
        || worker != contents.join("MacOS/llm-proxy-worker")
    {
        return Ok(false);
    }
    let invalid = || {
        AppError::new("startup_invalid_bundle", "The installed app's background-service files or configuration are missing or invalid. Reinstall the complete ICA proxy app in Applications and open that copy.")
    };
    let info = plist::Value::from_file(contents.join("Info.plist")).map_err(|_| invalid())?;
    let info = info.as_dictionary().ok_or_else(invalid)?;
    if info
        .get("CFBundleIdentifier")
        .and_then(plist::Value::as_string)
        != Some("app.llmproxy.desktop")
        || info
            .get("CFBundleExecutable")
            .and_then(plist::Value::as_string)
            != Some("llm-proxy")
    {
        return Err(invalid());
    }
    for binary in [worker.to_path_buf(), contents.join("MacOS/llm-proxy")] {
        let metadata = fs::metadata(binary).map_err(|_| invalid())?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(invalid());
        }
    }
    let agent = plist::Value::from_file(contents.join("Library/LaunchAgents").join(AGENT))
        .map_err(|_| invalid())?;
    let agent = agent.as_dictionary().ok_or_else(invalid)?;
    let arguments = agent
        .get("ProgramArguments")
        .and_then(plist::Value::as_array);
    if agent.get("Label").and_then(plist::Value::as_string) != Some("app.llmproxy.worker")
        || agent.get("BundleProgram").and_then(plist::Value::as_string)
            != Some("Contents/MacOS/llm-proxy-worker")
        || agent.get("RunAtLoad").and_then(plist::Value::as_boolean) != Some(true)
        || !arguments.is_some_and(|args| {
            args.len() == 2
                && args[0].as_string() == Some("llm-proxy-worker")
                && args[1].as_string() == Some("--managed")
        })
    {
        return Err(invalid());
    }
    Ok(true)
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

pub fn disable_enrolled(directory: &Path) -> Result<(), AppError> {
    if cfg!(debug_assertions) {
        return Ok(());
    }
    disable_enrolled_with_service(directory, &NativeService)
}

fn disable_enrolled_with_service(
    directory: &Path,
    service: &impl StartupService,
) -> Result<(), AppError> {
    // A fresh setup can run in the foreground without contacting ServiceManagement
    // or Task Scheduler. An earlier background attempt must be undone first.
    if load(directory)?.is_some_and(|registration| !registration.reset) {
        service.unregister()?;
        service.remove_legacy()?;
        save(
            directory,
            &Registration {
                fingerprint: String::new(),
                reset: true,
            },
        )?;
    }
    Ok(())
}

pub fn launch(worker: &Path, state: &str) -> Result<Option<std::process::ChildStdin>, AppError> {
    if state == "enabled" && !cfg!(debug_assertions) {
        platform::launch()?;
        return Ok(None);
    }
    let mut command = std::process::Command::new(worker);
    if state == "foreground" {
        command.arg("--foreground");
    }
    command
        .stdin(if state == "foreground" {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|_| error())?;
    let owner = child.stdin.take();
    // Reap the child independently. For foreground launches, the caller retains the
    // stdin pipe; closing it tells the worker to stop and drain its proxy listeners.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(owner)
}

pub fn open_settings() -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    {
        platform::open_settings();
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    Err(AppError::new(
        "startup_settings_unavailable",
        "Check the ICA proxy task in Windows Task Scheduler. This app does not require administrator permission for background startup.",
    ))
}

fn error() -> AppError {
    AppError::new("startup_registration_failed", "Automatic background startup could not be configured. Install the complete app, then select Check background startup to retry. The proxy can still run during this sign-in. On managed computers, ask IT to check background-app policies.")
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use objc2::{rc::autoreleasepool, rc::Retained};
    use objc2_foundation::{NSError, NSString};
    use objc2_service_management::{
        kSMErrorAuthorizationFailure, kSMErrorInvalidSignature, kSMErrorJobPlistNotFound,
        kSMErrorLaunchDeniedByUser, kSMErrorToolNotValid, SMAppService, SMAppServiceStatus,
    };

    fn agent() -> Retained<SMAppService> {
        // SAFETY: this constant names the LaunchAgent plist shipped in our signed app bundle.
        unsafe { SMAppService::agentServiceWithPlistName(&NSString::from_str(AGENT)) }
    }

    pub fn status() -> Result<ServiceState, AppError> {
        autoreleasepool(|_| {
            // SAFETY: the retained service is live; status has no preconditions.
            let status = unsafe { agent().status() };
            service_status(status)
        })
    }

    fn service_status(status: SMAppServiceStatus) -> Result<ServiceState, AppError> {
        if status == SMAppServiceStatus::Enabled {
            Ok(ServiceState::Enabled)
        } else if status == SMAppServiceStatus::RequiresApproval {
            Ok(ServiceState::ApprovalRequired)
        } else if status == SMAppServiceStatus::NotRegistered {
            Ok(ServiceState::Disabled)
        } else if status == SMAppServiceStatus::NotFound {
            Ok(ServiceState::Missing)
        } else {
            Err(AppError::new(
                "startup_status_unknown",
                format!("macOS returned an unrecognized background-service status ({}). Share this error message with the app publisher.", status.0),
            ))
        }
    }

    pub fn register(_worker: &Path) -> Result<(), AppError> {
        autoreleasepool(|_| {
            // SAFETY: registration is for our bundled per-user agent; OS consent remains enforced.
            unsafe { agent().registerAndReturnError() }.map_err(|native_error| {
                registration_error(&native_error.domain().to_string(), native_error.code())
            })
        })
    }

    fn registration_error(domain: &str, code: isize) -> AppError {
        // SMAppServiceErrorDomain's exported symbol requires macOS 15. Compare domain
        // values instead so this error path also works on our macOS 13/14 minimum.
        let service_domain = matches!(
            domain,
            "SMAppServiceErrorDomain" | "kSMErrorDomainFramework"
        );
        let (kind, guidance) = if service_domain && code == kSMErrorLaunchDeniedByUser as isize {
            (
                "startup_approval_required",
                "macOS requires approval for background startup. Select Open Login Items settings and allow ICA proxy under Allow in the Background, then return and select Check background startup.",
            )
        } else if service_domain && code == kSMErrorInvalidSignature as isize {
            (
                "startup_invalid_signature",
                "macOS rejected the background service's code signature. Reinstall the complete app in Applications. If it still fails, the app publisher must fix signing; granting background permission cannot repair an invalid signature.",
            )
        } else if service_domain
            && (code == kSMErrorJobPlistNotFound as isize || code == kSMErrorToolNotValid as isize)
        {
            (
                "startup_invalid_bundle",
                "macOS could not use the bundled background service. Install the complete ICA proxy app in Applications and open that installed copy, then select Check background startup.",
            )
        } else if service_domain && code == kSMErrorAuthorizationFailure as isize {
            (
                "startup_authorization_failed",
                "macOS refused background-service registration. Check Login Items settings and any organization policy, then select Check background startup. Contact IT if this Mac is managed.",
            )
        } else {
            (
                "startup_registration_failed",
                "macOS could not register the background service. Install the complete app in Applications, then select Check background startup to retry. If it still fails, share this error code with the app publisher.",
            )
        };
        let domain = if service_domain {
            "SMAppService"
        } else {
            "macOS"
        };
        // Only a known domain label and numeric code leave the native boundary. NSError's
        // descriptions/userInfo may contain paths or other unreviewed diagnostic details.
        AppError::new(kind, format!("{guidance} ({domain} error {code}.)"))
    }

    pub fn open_settings() {
        autoreleasepool(|_| {
            // SAFETY: this documented API opens Login Items; the user still grants approval.
            unsafe { SMAppService::openSystemSettingsLoginItems() };
        });
    }

    pub fn unregister() -> Result<(), AppError> {
        autoreleasepool(|_| {
            // A missing bundle/service must not prevent reset or uninstall from cleaning up.
            // SAFETY: the retained service is live; status has no preconditions.
            let state = unsafe { agent().status() };
            if state == SMAppServiceStatus::NotRegistered || state == SMAppServiceStatus::NotFound {
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn distinguishes_permission_installation_and_disabled_service_states() {
            assert_eq!(
                service_status(SMAppServiceStatus::Enabled).expect("enabled"),
                ServiceState::Enabled
            );
            assert_eq!(
                service_status(SMAppServiceStatus::RequiresApproval).expect("approval"),
                ServiceState::ApprovalRequired
            );
            assert_eq!(
                service_status(SMAppServiceStatus::NotRegistered).expect("unregistered"),
                ServiceState::Disabled
            );
            assert_eq!(
                service_status(SMAppServiceStatus::NotFound)
                    .expect("missing service is recoverable"),
                ServiceState::Missing
            );
        }

        #[test]
        fn registration_errors_do_not_mistake_signing_failures_for_user_denial() {
            assert_eq!(
                registration_error(
                    "SMAppServiceErrorDomain",
                    kSMErrorLaunchDeniedByUser as isize
                )
                .code,
                "startup_approval_required"
            );
            let signature =
                registration_error("SMAppServiceErrorDomain", kSMErrorInvalidSignature as isize);
            assert_eq!(signature.code, "startup_invalid_signature");
            assert!(signature
                .message
                .contains("cannot repair an invalid signature"));
            assert_eq!(
                registration_error("NSPOSIXErrorDomain", kSMErrorLaunchDeniedByUser as isize).code,
                "startup_registration_failed"
            );
            assert_eq!(
                registration_error(
                    "kSMErrorDomainFramework",
                    kSMErrorLaunchDeniedByUser as isize
                )
                .code,
                "startup_approval_required"
            );
        }

        #[test]
        fn unknown_native_status_reports_only_a_safe_numeric_diagnostic() {
            let failure = service_status(SMAppServiceStatus(999)).expect_err("unknown status");
            assert_eq!(failure.code, "startup_status_unknown");
            assert!(failure.message.contains("999"));
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

    pub fn status() -> Result<ServiceState, AppError> {
        let output = query()?;
        if !output.status.success() {
            return Ok(ServiceState::Disabled);
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
            ServiceState::Disabled
        } else {
            ServiceState::Enabled
        })
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
  <RegistrationInfo><Description>Runs the ICA proxy background worker after sign-in.</Description></RegistrationInfo>
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
    fn new_installation_defers_os_registration_until_final_setup_action() {
        let directory = tempfile::tempdir().expect("isolated new installation");
        let state = initialize(
            directory.path(),
            &directory.path().join("missing-worker"),
            Intent::Inspect,
        )
        .expect("no native registration or executable access during partial setup");
        assert_eq!(
            state,
            if cfg!(debug_assertions) {
                "development"
            } else {
                "setup_required"
            }
        );
        assert!(load(directory.path()).expect("read marker").is_none());
        save(
            directory.path(),
            &Registration {
                fingerprint: String::new(),
                reset: true,
            },
        )
        .expect("reset");
        let state = initialize(
            directory.path(),
            &directory.path().join("missing-worker"),
            Intent::Inspect,
        )
        .expect("resume after reset");
        assert_eq!(
            state,
            if cfg!(debug_assertions) {
                "development"
            } else {
                "setup_required"
            }
        );
        assert!(
            load(directory.path())
                .expect("read marker")
                .expect("preserved reset")
                .reset
        );
    }

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
        assert!(!needs_worker_handoff(directory.path(), Intent::Inspect)
            .expect("relaunch preserves reset"));
        assert!(
            !needs_worker_handoff(directory.path(), Intent::Repair).expect("check preserves reset")
        );
        if !cfg!(debug_assertions) {
            assert!(needs_worker_handoff(directory.path(), Intent::Enroll)
                .expect("explicit setup enrolls"));
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
        for intent in [Intent::Inspect, Intent::Enroll, Intent::Repair] {
            assert!(!needs_worker_handoff(directory.path(), intent).expect("existing enrollment"));
        }
    }

    struct Fixture {
        _temporary: tempfile::TempDir,
        directory: PathBuf,
        worker: PathBuf,
    }

    impl Fixture {
        fn new(completed: bool) -> Self {
            let temporary = tempfile::tempdir().expect("isolated startup fixture");
            let directory = temporary.path().join("settings");
            fs::create_dir_all(&directory).expect("settings directory");
            fs::write(
                directory.join("settings.json"),
                serde_json::to_vec(&serde_json::json!({
                    "schemaVersion": 1, "localPort": 8787, "startAtLogin": true,
                    "onboardingComplete": completed, "providers": []
                }))
                .expect("settings JSON"),
            )
            .expect("save settings");
            let contents = temporary.path().join("ICA proxy.app/Contents");
            fs::create_dir_all(contents.join("MacOS")).expect("binary directory");
            let worker = contents.join("MacOS/llm-proxy-worker");
            for binary in [&worker, &contents.join("MacOS/llm-proxy")] {
                fs::write(binary, b"fake executable for registration policy tests")
                    .expect("binary");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(binary, fs::Permissions::from_mode(0o755))
                        .expect("executable");
                }
            }
            #[cfg(target_os = "macos")]
            {
                let info: plist::Value = serde_json::from_value(serde_json::json!({
                    "CFBundleIdentifier": "app.llmproxy.desktop", "CFBundleExecutable": "llm-proxy"
                }))
                .expect("bundle metadata");
                info.to_file_xml(contents.join("Info.plist"))
                    .expect("bundle metadata plist");
                fs::create_dir_all(contents.join("Library/LaunchAgents")).expect("agent directory");
                fs::write(
                    contents.join("Library/LaunchAgents").join(AGENT),
                    include_str!("../macos/app.llmproxy.worker.plist"),
                )
                .expect("agent plist");
            }
            Self {
                _temporary: temporary,
                directory,
                worker,
            }
        }

        fn marker(&self, fingerprint: &str, reset: bool) {
            save(
                &self.directory,
                &Registration {
                    fingerprint: fingerprint.into(),
                    reset,
                },
            )
            .expect("marker");
        }

        fn fingerprint(&self) -> String {
            proxy_service::binary_fingerprint(&self.worker).expect("worker fingerprint")
        }

        fn initialize(&self, intent: Intent, service: &FakeService) -> Result<String, AppError> {
            initialize_with_service(&self.directory, &self.worker, intent, service)
        }
    }

    struct FakeService {
        states: std::cell::RefCell<std::collections::VecDeque<ServiceState>>,
        calls: std::cell::RefCell<Vec<&'static str>>,
        registration: Result<(), AppError>,
        unregistration: Result<(), AppError>,
    }

    impl FakeService {
        fn new(states: impl IntoIterator<Item = ServiceState>) -> Self {
            Self {
                states: std::cell::RefCell::new(states.into_iter().collect()),
                calls: std::cell::RefCell::new(Vec::new()),
                registration: Ok(()),
                unregistration: Ok(()),
            }
        }

        fn assert_calls(&self, expected: &[&str]) {
            assert_eq!(*self.calls.borrow(), expected);
            assert!(
                self.states.borrow().is_empty(),
                "all expected status reads performed"
            );
        }
    }

    impl StartupService for FakeService {
        fn status(&self) -> Result<ServiceState, AppError> {
            self.calls.borrow_mut().push("status");
            Ok(self
                .states
                .borrow_mut()
                .pop_front()
                .expect("unexpected status read"))
        }

        fn register(&self, _worker: &Path) -> Result<(), AppError> {
            self.calls.borrow_mut().push("register");
            self.registration.clone()
        }

        fn unregister(&self) -> Result<(), AppError> {
            self.calls.borrow_mut().push("unregister");
            self.unregistration.clone()
        }

        fn remove_legacy(&self) -> Result<(), AppError> {
            self.calls.borrow_mut().push("remove_legacy");
            Ok(())
        }
    }

    #[test]
    fn foreground_preference_bypasses_native_startup_and_bundle_validation() {
        for completed in [false, true] {
            let fixture = Fixture::new(completed);
            fs::write(
                fixture.directory.join("settings.json"),
                serde_json::to_vec(&serde_json::json!({
                    "schemaVersion": 1, "localPort": 8787, "startAtLogin": true,
                    "onboardingComplete": completed, "providers": [], "runInBackground": false
                }))
                .expect("foreground settings"),
            )
            .expect("save preference");
            fixture.marker("previous-enrollment", false);
            fs::remove_file(&fixture.worker).expect("missing bundle worker");
            for intent in [Intent::Inspect, Intent::Repair, Intent::Enroll] {
                let service = FakeService::new([]);
                assert_eq!(
                    fixture.initialize(intent, &service).expect("foreground"),
                    "foreground"
                );
                service.assert_calls(&[]);
                assert_eq!(
                    initialize(&fixture.directory, &fixture.worker, intent).expect("public policy"),
                    "foreground"
                );
            }
            assert_eq!(
                load(&fixture.directory)
                    .expect("marker")
                    .expect("preserved")
                    .fingerprint,
                "previous-enrollment"
            );
        }
    }

    #[test]
    fn foreground_choice_without_active_enrollment_never_calls_native_startup() {
        let fixture = Fixture::new(false);
        let service = FakeService::new([]);
        disable_enrolled_with_service(&fixture.directory, &service).expect("fresh setup");
        assert!(load(&fixture.directory).expect("marker").is_none());
        fixture.marker("", true);
        disable_enrolled_with_service(&fixture.directory, &service).expect("already disabled");
        assert!(
            load(&fixture.directory)
                .expect("marker")
                .expect("reset")
                .reset
        );
        service.assert_calls(&[]);
    }

    #[test]
    fn foreground_choice_removes_earlier_background_enrollment() {
        let fixture = Fixture::new(false);
        fixture.marker(&fixture.fingerprint(), false);
        let service = FakeService::new([]);
        disable_enrolled_with_service(&fixture.directory, &service).expect("remove enrollment");
        service.assert_calls(&["unregister", "remove_legacy"]);
        let marker = load(&fixture.directory).expect("marker").expect("saved");
        assert!(marker.reset);
        assert!(marker.fingerprint.is_empty());
    }

    #[test]
    fn failed_foreground_switch_preserves_background_enrollment_for_retry() {
        let fixture = Fixture::new(false);
        let fingerprint = fixture.fingerprint();
        fixture.marker(&fingerprint, false);
        let mut service = FakeService::new([]);
        service.unregistration = Err(AppError::new("fixture_denied", "Simulated denial"));
        assert_eq!(
            disable_enrolled_with_service(&fixture.directory, &service)
                .expect_err("unregister failed")
                .code,
            "fixture_denied"
        );
        service.assert_calls(&["unregister"]);
        let marker = load(&fixture.directory)
            .expect("marker")
            .expect("preserved");
        assert!(!marker.reset);
        assert_eq!(marker.fingerprint, fingerprint);
    }

    #[cfg(unix)]
    #[test]
    fn foreground_launch_keeps_stdin_open_until_the_owner_is_dropped() {
        use std::{
            os::unix::fs::PermissionsExt,
            time::{Duration, Instant},
        };
        let directory = tempfile::tempdir().expect("isolated foreground executable");
        let worker = directory.path().join("fixture-worker");
        fs::write(
            &worker,
            "#!/bin/sh\n[ \"$1\" = \"--foreground\" ] || exit 7\nprintf ready > \"$0.ready\"\ncat >/dev/null\nprintf stopped > \"$0.done\"\n",
        ).expect("fixture executable");
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o755)).expect("permissions");
        let wait_for = |name| {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !directory.path().join(name).exists() {
                assert!(Instant::now() < deadline, "worker did not create {name}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let owner = launch(&worker, "foreground")
            .expect("launch")
            .expect("owner pipe");
        wait_for("fixture-worker.ready");
        std::thread::sleep(Duration::from_millis(30));
        assert!(!directory.path().join("fixture-worker.done").exists());
        drop(owner);
        wait_for("fixture-worker.done");
    }

    #[test]
    fn ordinary_inspection_never_repairs_a_missing_service() {
        for marked in [false, true] {
            let fixture = Fixture::new(true);
            if marked {
                fixture.marker(&fixture.fingerprint(), false);
            }
            let service = FakeService::new([ServiceState::Missing]);
            assert_eq!(
                fixture
                    .initialize(Intent::Inspect, &service)
                    .expect_err("explicit repair needed")
                    .code,
                "startup_service_not_found"
            );
            service.assert_calls(&["status"]);
            assert_eq!(load(&fixture.directory).expect("marker").is_some(), marked);
        }
    }

    #[test]
    fn explicit_retry_repairs_missing_service_even_with_an_existing_marker() {
        let fixture = Fixture::new(true);
        fixture.marker(&fixture.fingerprint(), false);
        let service = FakeService::new([ServiceState::Missing, ServiceState::Enabled]);
        assert_eq!(
            fixture
                .initialize(Intent::Repair, &service)
                .expect("registered"),
            "enabled"
        );
        service.assert_calls(&["status", "register", "status", "remove_legacy"]);
        let marker = load(&fixture.directory)
            .expect("read")
            .expect("verified marker");
        assert_eq!(marker.fingerprint, fixture.fingerprint());
        assert!(!marker.reset);
    }

    #[test]
    fn final_setup_enrolls_a_missing_service_after_fresh_install_or_reset() {
        for reset in [false, true] {
            let fixture = Fixture::new(false);
            if reset {
                fixture.marker("", true);
            }
            let service = FakeService::new([ServiceState::Missing, ServiceState::ApprovalRequired]);
            assert_eq!(
                fixture
                    .initialize(Intent::Enroll, &service)
                    .expect("enrolled"),
                "approval_required"
            );
            service.assert_calls(&["status", "register", "status", "remove_legacy"]);
            assert!(
                !load(&fixture.directory)
                    .expect("read")
                    .expect("saved marker")
                    .reset
            );
        }
    }

    #[test]
    fn verified_registration_status_takes_precedence_over_native_error() {
        for state in [ServiceState::Enabled, ServiceState::ApprovalRequired] {
            let fixture = Fixture::new(true);
            let mut service = FakeService::new([ServiceState::Missing, state]);
            service.registration = Err(AppError::new("startup_approval_required", "native denial"));
            assert_eq!(
                fixture
                    .initialize(Intent::Repair, &service)
                    .expect("authoritative status"),
                state.display().expect("registered state")
            );
            service.assert_calls(&["status", "register", "status", "remove_legacy"]);
            assert!(load(&fixture.directory).expect("read").is_some());
        }
    }

    #[test]
    fn failed_repair_preserves_native_error_and_previous_marker() {
        let fixture = Fixture::new(true);
        fixture.marker("previous-worker", false);
        let mut service = FakeService::new([ServiceState::Missing, ServiceState::Missing]);
        service.registration = Err(AppError::new(
            "startup_invalid_signature",
            "sanitized native error 2",
        ));
        let failure = fixture
            .initialize(Intent::Repair, &service)
            .expect_err("native failure");
        assert_eq!(failure.code, "startup_invalid_signature");
        assert_eq!(failure.message, "sanitized native error 2");
        service.assert_calls(&["status", "register", "status"]);
        assert_eq!(
            load(&fixture.directory)
                .expect("read")
                .expect("previous marker")
                .fingerprint,
            "previous-worker"
        );
    }

    #[test]
    fn unverified_registration_never_saves_a_success_marker_or_retries_in_a_loop() {
        for (state, code) in [
            (ServiceState::Missing, "startup_service_not_found"),
            (ServiceState::Disabled, "startup_registration_incomplete"),
        ] {
            let fixture = Fixture::new(true);
            let service = FakeService::new([ServiceState::Missing, state]);
            assert_eq!(
                fixture
                    .initialize(Intent::Repair, &service)
                    .expect_err("not registered")
                    .code,
                code
            );
            service.assert_calls(&["status", "register", "status"]);
            assert!(load(&fixture.directory).expect("read").is_none());
        }
    }

    #[test]
    fn existing_os_opt_out_is_preserved_even_after_upgrade_or_explicit_retry() {
        for intent in [Intent::Inspect, Intent::Repair, Intent::Enroll] {
            let fixture = Fixture::new(true);
            fixture.marker("previous-worker", false);
            let service = FakeService::new([ServiceState::Disabled]);
            assert_eq!(
                fixture.initialize(intent, &service).expect("OS opt out"),
                "disabled"
            );
            service.assert_calls(&["status"]);
            assert_eq!(
                load(&fixture.directory)
                    .expect("read")
                    .expect("previous marker")
                    .fingerprint,
                "previous-worker"
            );
        }
    }

    #[test]
    fn incomplete_setup_and_reset_prevent_early_repair() {
        for completed in [false, true] {
            for intent in [Intent::Inspect, Intent::Repair] {
                let fixture = Fixture::new(completed);
                fixture.marker("", true);
                let service = FakeService::new([]);
                assert_eq!(
                    fixture.initialize(intent, &service).expect("deferred"),
                    if completed {
                        "disabled"
                    } else {
                        "setup_required"
                    }
                );
                service.assert_calls(&[]);
                assert!(
                    load(&fixture.directory)
                        .expect("read")
                        .expect("reset marker")
                        .reset
                );
            }
        }
        let fixture = Fixture::new(false);
        let service = FakeService::new([]);
        assert_eq!(
            fixture
                .initialize(Intent::Repair, &service)
                .expect("fresh setup"),
            "setup_required"
        );
        service.assert_calls(&[]);
    }

    #[test]
    fn skipping_the_wizard_does_not_enroll_startup_until_an_explicit_start() {
        for intent in [Intent::Inspect, Intent::Repair] {
            let fixture = Fixture::new(false);
            let path = fixture.directory.join("settings.json");
            let mut settings: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).expect("settings")).expect("JSON");
            settings["onboardingSkipped"] = true.into();
            fs::write(path, serde_json::to_vec(&settings).expect("updated JSON"))
                .expect("save skip");
            let service = FakeService::new([]);
            assert_eq!(
                fixture
                    .initialize(intent, &service)
                    .expect("deferred enrollment"),
                "setup_required"
            );
            service.assert_calls(&[]);
            assert!(load(&fixture.directory).expect("marker").is_none());
        }
    }

    #[test]
    fn partial_enrollment_remains_inspectable_without_early_registration_or_upgrade() {
        for state in [
            ServiceState::Missing,
            ServiceState::Enabled,
            ServiceState::ApprovalRequired,
        ] {
            let fixture = Fixture::new(false);
            fixture.marker("previous-worker", false);
            let service = FakeService::new([state]);
            let result = fixture.initialize(Intent::Repair, &service);
            if state == ServiceState::Missing {
                assert_eq!(
                    result.expect_err("final setup owns repair").code,
                    "startup_service_not_found"
                );
                service.assert_calls(&["status"]);
            } else {
                assert_eq!(
                    result.expect("existing enrollment"),
                    state.display().expect("display")
                );
                service.assert_calls(&["status", "remove_legacy"]);
            }
            assert_eq!(
                load(&fixture.directory)
                    .expect("read")
                    .expect("old marker")
                    .fingerprint,
                "previous-worker"
            );
        }
    }

    #[test]
    fn enabled_upgrade_replaces_registration_then_verifies_before_saving_fingerprint() {
        let fixture = Fixture::new(true);
        fixture.marker("previous-worker", false);
        let service = FakeService::new([ServiceState::Enabled, ServiceState::Enabled]);
        assert_eq!(
            fixture
                .initialize(Intent::Inspect, &service)
                .expect("upgrade"),
            "enabled"
        );
        service.assert_calls(&[
            "status",
            "unregister",
            "register",
            "status",
            "remove_legacy",
        ]);
        assert_eq!(
            load(&fixture.directory)
                .expect("read")
                .expect("new marker")
                .fingerprint,
            fixture.fingerprint()
        );
    }

    #[test]
    fn existing_verified_service_is_adopted_without_redundant_registration() {
        for state in [ServiceState::Enabled, ServiceState::ApprovalRequired] {
            let fixture = Fixture::new(true);
            let service = FakeService::new([state]);
            assert_eq!(
                fixture
                    .initialize(Intent::Inspect, &service)
                    .expect("adopted"),
                state.display().expect("registered")
            );
            service.assert_calls(&["status", "remove_legacy"]);
            assert_eq!(
                load(&fixture.directory)
                    .expect("read")
                    .expect("adopted marker")
                    .fingerprint,
                fixture.fingerprint()
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn invalid_bundle_metadata_and_agent_configuration_fail_before_native_calls() {
        for (relative, key, replacement) in [
            ("Info.plist", "CFBundleIdentifier", "another.app"),
            ("Info.plist", "CFBundleExecutable", "another-binary"),
            (
                "Library/LaunchAgents/app.llmproxy.worker.plist",
                "Label",
                "another.agent",
            ),
            (
                "Library/LaunchAgents/app.llmproxy.worker.plist",
                "BundleProgram",
                "/outside/worker",
            ),
            (
                "Library/LaunchAgents/app.llmproxy.worker.plist",
                "ProgramArguments",
                "invalid",
            ),
            (
                "Library/LaunchAgents/app.llmproxy.worker.plist",
                "RunAtLoad",
                "invalid",
            ),
        ] {
            let fixture = Fixture::new(true);
            let contents = fixture
                .worker
                .parent()
                .expect("MacOS")
                .parent()
                .expect("Contents");
            let path = contents.join(relative);
            let mut value = plist::Value::from_file(&path).expect("valid fixture");
            value
                .as_dictionary_mut()
                .expect("dictionary")
                .insert(key.into(), replacement.into());
            value.to_file_xml(&path).expect("invalid fixture");
            let service = FakeService::new([]);
            assert_eq!(
                fixture
                    .initialize(Intent::Repair, &service)
                    .expect_err("invalid bundle")
                    .code,
                "startup_invalid_bundle"
            );
            service.assert_calls(&[]);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn missing_corrupt_or_nonexecutable_bundle_files_fail_before_native_calls() {
        use std::os::unix::fs::PermissionsExt;
        for relative in [
            "Info.plist",
            "Library/LaunchAgents/app.llmproxy.worker.plist",
            "MacOS/llm-proxy",
            "MacOS/llm-proxy-worker",
        ] {
            for corrupt in [false, true] {
                let fixture = Fixture::new(true);
                let path = fixture
                    .worker
                    .parent()
                    .expect("MacOS")
                    .parent()
                    .expect("Contents")
                    .join(relative);
                if !corrupt {
                    fs::remove_file(&path).expect("missing file");
                } else if relative.ends_with(".plist") {
                    fs::write(&path, "broken plist").expect("corrupt file");
                } else {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
                        .expect("not executable");
                }
                let service = FakeService::new([]);
                assert_eq!(
                    fixture
                        .initialize(Intent::Repair, &service)
                        .expect_err("invalid bundle")
                        .code,
                    "startup_invalid_bundle"
                );
                service.assert_calls(&[]);
            }
        }
    }
}
