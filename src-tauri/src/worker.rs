use std::{future::Future, time::Duration};

use proxy_service::error::AppError;

pub(crate) struct Identity {
    pub fingerprint: String,
    pub managed: bool,
    pub foreground: bool,
}

// Keep startup/recovery testable without using the real worker or OS registration.
pub(crate) trait Host: Sync {
    fn identity(&self) -> impl Future<Output = Result<Identity, AppError>> + Send;
    fn stop(&self) -> impl Future<Output = Result<(), AppError>> + Send;
    fn launch(&self, state: &str) -> impl Future<Output = Result<(), AppError>> + Send;
    fn recover(&self) -> impl Future<Output = Result<String, AppError>> + Send;
    fn pause(&self, attempt: usize) -> impl Future<Output = ()> + Send;
}

#[derive(Default)]
pub(crate) struct Connection {
    recovery_attempted: bool,
}

impl Connection {
    pub async fn connect(
        &mut self,
        host: &impl Host,
        expected: &str,
        state: &str,
        owns_foreground: bool,
        can_recover: bool,
    ) -> Result<(), AppError> {
        match host.identity().await {
            Ok(identity) => {
                if matches(&identity, expected, state) && (state != "foreground" || owns_foreground)
                {
                    return Ok(());
                }
                // Never send settings or secrets to another version or run mode.
                host.stop().await?;
            }
            Err(error) if error.code == "worker_unavailable" => {}
            Err(error) => return Err(error),
        }
        let result = launch_and_wait(host, expected, state).await;
        if result.is_ok() || state != "enabled" || !can_recover || self.recovery_attempted {
            return result;
        }

        // Registration may be enabled yet still resolve an obsolete app bundle.
        // Limit automatic recovery to once per settings session, including failures.
        self.recovery_attempted = true;
        let state = host.recover().await?;
        match state.as_str() {
            "enabled" => launch_and_wait(host, expected, &state).await.map_err(|error| {
                if error.code == "worker_start_failed" {
                    AppError::new("worker_service_unavailable", "macOS still could not start the installed proxy worker after refreshing background startup. Quit ICA proxy and reopen the copy in Applications, then select Check background startup. If it still fails, reinstall the complete app without resetting your saved data.")
                } else {
                    error
                }
            }),
            "approval_required" => Err(AppError::new("startup_approval_required", "macOS requires approval for the refreshed background service. Allow ICA proxy in Login Items & Extensions, then select Check background startup.")),
            "disabled" => Err(AppError::new("startup_disabled", "Background startup is disabled. Your OS or reset preference has been preserved. Reopen settings to run the proxy manually.")),
            _ => Err(AppError::new("startup_recovery_deferred", "Background startup could not be refreshed with the current setup. Reopen settings and finish setup or select Check background startup.")),
        }
    }
}

fn matches(identity: &Identity, expected: &str, state: &str) -> bool {
    identity.fingerprint == expected
        && (state != "enabled" || identity.managed)
        && identity.foreground == (state == "foreground")
}

async fn launch_and_wait(host: &impl Host, expected: &str, state: &str) -> Result<(), AppError> {
    host.launch(state).await?;
    // Identity never needs Keychain approval. Bound the entire handshake, rather
    // than inheriting the five-minute timeout used by interactive IPC commands.
    let ready = tokio::time::timeout(Duration::from_secs(10), async {
        for attempt in 0..40 {
            match host.identity().await {
                Ok(identity) if matches(&identity, expected, state) => return Ok(()),
                Err(error) if error.code != "worker_unavailable" => return Err(error),
                _ => {}
            }
            host.pause(attempt).await;
        }
        Err(start_error())
    })
    .await;
    ready.unwrap_or_else(|_| Err(start_error()))
}

fn start_error() -> AppError {
    AppError::new("worker_start_failed", "The installed proxy worker did not respond. Quit ICA proxy and reopen the installed app. If it still fails, reinstall the complete app without resetting your saved data.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};

    struct Fixture {
        probes: Mutex<VecDeque<Result<Identity, AppError>>>,
        launches: Mutex<Vec<String>>,
        stops: Mutex<usize>,
        repairs: Mutex<usize>,
        repaired: Result<String, AppError>,
        ready_after_repair: bool,
        launch_error: bool,
    }

    impl Default for Fixture {
        fn default() -> Self {
            Self {
                probes: Mutex::new(VecDeque::new()),
                launches: Mutex::new(Vec::new()),
                stops: Mutex::new(0),
                repairs: Mutex::new(0),
                repaired: Ok("enabled".into()),
                ready_after_repair: false,
                launch_error: false,
            }
        }
    }

    fn identity(fingerprint: &str, managed: bool, foreground: bool) -> Identity {
        Identity {
            fingerprint: fingerprint.into(),
            managed,
            foreground,
        }
    }

    impl Host for Fixture {
        async fn identity(&self) -> Result<Identity, AppError> {
            if let Some(result) = self.probes.lock().expect("fixture lock").pop_front() {
                return result;
            }
            if self.ready_after_repair && *self.repairs.lock().expect("fixture lock") > 0 {
                return Ok(identity("installed", true, false));
            }
            Err(AppError::new("worker_unavailable", "fixture unavailable"))
        }
        async fn stop(&self) -> Result<(), AppError> {
            *self.stops.lock().expect("fixture lock") += 1;
            Ok(())
        }
        async fn launch(&self, state: &str) -> Result<(), AppError> {
            self.launches
                .lock()
                .expect("fixture lock")
                .push(state.into());
            if self.launch_error && *self.repairs.lock().expect("fixture lock") == 0 {
                return Err(AppError::new(
                    "startup_registration_failed",
                    "launch rejected",
                ));
            }
            Ok(())
        }
        async fn recover(&self) -> Result<String, AppError> {
            *self.repairs.lock().expect("fixture lock") += 1;
            self.repaired.clone()
        }
        async fn pause(&self, _attempt: usize) {}
    }

    #[tokio::test]
    async fn healthy_existing_worker_is_never_restarted_or_reregistered() {
        let fixture = Fixture::default();
        fixture
            .probes
            .lock()
            .expect("fixture lock")
            .push_back(Ok(identity("installed", true, false)));
        Connection::default()
            .connect(&fixture, "installed", "enabled", false, true)
            .await
            .expect("connected");
        assert!(fixture.launches.lock().expect("fixture lock").is_empty());
        assert_eq!(*fixture.stops.lock().expect("fixture lock"), 0);
        assert_eq!(*fixture.repairs.lock().expect("fixture lock"), 0);
    }

    #[tokio::test]
    async fn stale_service_recovers_after_timeout_or_native_launch_failure() {
        for launch_error in [false, true] {
            let fixture = Fixture {
                ready_after_repair: true,
                launch_error,
                ..Fixture::default()
            };
            Connection::default()
                .connect(&fixture, "installed", "enabled", false, true)
                .await
                .expect("recovered");
            assert_eq!(*fixture.repairs.lock().expect("fixture lock"), 1);
            assert_eq!(
                *fixture.launches.lock().expect("fixture lock"),
                ["enabled", "enabled"]
            );
        }
    }

    #[tokio::test]
    async fn successful_launch_and_upgrade_handoff_do_not_trigger_recovery() {
        for previous in [
            Err(AppError::new("worker_unavailable", "not running")),
            Ok(identity("previous-version", true, false)),
        ] {
            let old_worker = previous.is_ok();
            let fixture = Fixture::default();
            fixture
                .probes
                .lock()
                .expect("fixture lock")
                .extend([previous, Ok(identity("installed", true, false))]);
            Connection::default()
                .connect(&fixture, "installed", "enabled", false, true)
                .await
                .expect("installed worker connected");
            assert_eq!(
                *fixture.stops.lock().expect("fixture lock"),
                usize::from(old_worker)
            );
            assert_eq!(*fixture.repairs.lock().expect("fixture lock"), 0);
        }
    }

    #[tokio::test]
    async fn recovery_never_accepts_an_old_worker_as_success() {
        let fixture = Fixture::default();
        fixture
            .probes
            .lock()
            .expect("fixture lock")
            .extend((0..100).map(|_| Ok(identity("previous-version", true, false))));
        assert_eq!(
            Connection::default()
                .connect(&fixture, "installed", "enabled", false, true)
                .await
                .expect_err("wrong worker after repair")
                .code,
            "worker_service_unavailable"
        );
        assert_eq!(*fixture.repairs.lock().expect("fixture lock"), 1);
    }

    #[tokio::test]
    async fn foreground_worker_requires_ownership_and_replacement_keeps_foreground_mode() {
        for owned in [false, true] {
            let fixture = Fixture::default();
            fixture.probes.lock().expect("fixture lock").extend([
                Ok(identity("installed", false, true)),
                Ok(identity("installed", false, true)),
            ]);
            Connection::default()
                .connect(&fixture, "installed", "foreground", owned, true)
                .await
                .expect("owned worker");
            assert_eq!(
                *fixture.stops.lock().expect("fixture lock"),
                usize::from(!owned)
            );
            assert_eq!(
                fixture.launches.lock().expect("fixture lock").len(),
                usize::from(!owned)
            );
            assert_eq!(*fixture.repairs.lock().expect("fixture lock"), 0);
            assert!(fixture
                .launches
                .lock()
                .expect("fixture lock")
                .iter()
                .all(|state| state == "foreground"));
        }
    }

    #[tokio::test]
    async fn failed_service_is_repaired_only_once_across_reconnects() {
        let fixture = Fixture::default();
        let mut connection = Connection::default();
        assert_eq!(
            connection
                .connect(&fixture, "installed", "enabled", false, true)
                .await
                .expect_err("still unavailable")
                .code,
            "worker_service_unavailable"
        );
        assert!(connection
            .connect(&fixture, "installed", "enabled", false, true)
            .await
            .is_err());
        assert_eq!(*fixture.repairs.lock().expect("fixture lock"), 1);
        assert_eq!(fixture.launches.lock().expect("fixture lock").len(), 3);
    }

    #[tokio::test]
    async fn repair_denial_preserves_native_error_and_never_loops() {
        let fixture = Fixture {
            repaired: Err(AppError::new(
                "startup_invalid_signature",
                "native signature failure",
            )),
            ..Fixture::default()
        };
        let mut connection = Connection::default();
        let error = connection
            .connect(&fixture, "installed", "enabled", false, true)
            .await
            .expect_err("denied");
        assert_eq!(error.code, "startup_invalid_signature");
        assert_eq!(error.message, "native signature failure");
        assert!(connection
            .connect(&fixture, "installed", "enabled", false, true)
            .await
            .is_err());
        assert_eq!(*fixture.repairs.lock().expect("fixture lock"), 1);
    }

    #[tokio::test]
    async fn recovery_never_launches_through_new_denial_or_opt_out() {
        for state in ["approval_required", "disabled", "setup_required"] {
            let fixture = Fixture {
                repaired: Ok(state.into()),
                ..Fixture::default()
            };
            assert!(Connection::default()
                .connect(&fixture, "installed", "enabled", false, true)
                .await
                .is_err());
            assert_eq!(*fixture.launches.lock().expect("fixture lock"), ["enabled"]);
            assert_eq!(*fixture.repairs.lock().expect("fixture lock"), 1);
        }
    }

    #[tokio::test]
    async fn foreground_disabled_and_development_launches_never_repair() {
        for (state, can_recover) in [
            ("foreground", true),
            ("disabled", true),
            ("setup_required", true),
            ("enabled", false),
        ] {
            let fixture = Fixture::default();
            assert!(Connection::default()
                .connect(&fixture, "installed", state, false, can_recover)
                .await
                .is_err());
            assert_eq!(*fixture.repairs.lock().expect("fixture lock"), 0);
        }
    }

    #[tokio::test]
    async fn matching_fingerprint_with_wrong_mode_is_replaced_before_connecting() {
        for (managed, foreground) in [(false, false), (false, true)] {
            let fixture = Fixture::default();
            fixture.probes.lock().expect("fixture lock").extend([
                Ok(identity("installed", managed, foreground)),
                Ok(identity("installed", true, false)),
            ]);
            Connection::default()
                .connect(&fixture, "installed", "enabled", false, true)
                .await
                .expect("matching managed worker");
            assert_eq!(*fixture.stops.lock().expect("fixture lock"), 1);
            assert_eq!(*fixture.launches.lock().expect("fixture lock"), ["enabled"]);
            assert_eq!(*fixture.repairs.lock().expect("fixture lock"), 0);
        }
    }
}
