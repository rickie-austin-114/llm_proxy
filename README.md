# ICA proxy

**ICA proxy** is a lightweight, open-source desktop app that keeps your Claude and OpenAI keys in the OS credential store and forwards native API requests through localhost. Built with Tauri 2, Rust, and Svelte. Licensed under the [MIT License](LICENSE); you can inspect, modify, and redistribute the source under its terms.

## Project documents

- [Requirements](REQUIREMENTS.md): supported platforms, GUI setup, security, startup, and resource budgets.
- [Architecture](ARCHITECTURE.md): framework decisions, API routing, lifecycle, and release validation.
- [Coding style](CODING_STYLE.md): implementation conventions, component boundaries, and checks.

## Install and configure

The v0.2.4 preview fixes macOS worker startup when an enabled background service still refers to an older app after an update. It refreshes an existing enabled registration once, verifies the installed worker, and preserves saved settings, keys, certificate trust, and run preferences. It also includes the foreground startup option and blue macOS Keychain guidance panel from v0.2.3. The [older Word installation guide](docs/LLM_Proxy_PowerPoint_Installation_Guide.doc) includes PowerPoint screenshots, but shows the previous two-button HTTPS setup and tray lifecycle.

Download the **ICA proxy v0.2.4 preview** installer for your computer:

| Computer                 | Download                                                                                                                            |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| Mac with Apple Silicon   | [Download DMG](https://github.com/rickie-austin-114/llm_proxy/releases/download/v0.2.4/ICA-Proxy-0.2.4-macOS-Apple-Silicon.dmg)     |
| Mac with Intel processor | [Download DMG](https://github.com/rickie-austin-114/llm_proxy/releases/download/v0.2.4/ICA-Proxy-0.2.4-macOS-Intel.dmg)             |
| Windows x64              | [Download setup EXE](https://github.com/rickie-austin-114/llm_proxy/releases/download/v0.2.4/ICA-Proxy-0.2.4-Windows-x64-Setup.exe) |

These are development packages: the Mac apps have an ad-hoc signature without Apple notarization, and the Windows installer has no publisher signature. Signed production releases and the platform acceptance checks below are still pending. See the [release notes](https://github.com/rickie-austin-114/llm_proxy/releases/tag/v0.2.4) for limitations and checksums.

The intended graphical installation is:

- **macOS 13 or newer:** open the disk image for your processor, then drag **ICA proxy** into Applications. Separate native builds cover Intel and Apple Silicon.
- **Windows 11 x64:** run the setup executable. It installs for the current user and bootstraps WebView2 if missing; the runtime download requires internet access.

End users do not need Node.js, Rust, a package manager, or a terminal.

1. Open **ICA proxy**. The first-run wizard starts automatically on a new installation or after resetting app data.
2. **Provider key:** choose Claude or OpenAI, paste your API key, and select **Save key and continue**. Keep the default endpoint, or expand **Provider endpoint** to change it. One provider is sufficient; add the other or test its connection in settings later.
3. **Trusted HTTPS:** select **Set up HTTPS and trust certificate** and approve the operating system prompt. The wizard verifies trust before continuing. If approval is canceled, retry with the saved certificate.
4. **Start proxy:** select **Start proxy and enable background startup**. If macOS needs approval, select **Open Login Items settings**, enable ICA proxy under **Allow in the Background**, return, and select **Check approval and finish setup**. The proxy can run during this sign-in while approval is pending; this choice finishes setup only after background startup is enabled. Choose **Start without background startup** to finish setup and run only while ICA proxy is open, without enabling sign-in startup.
5. Copy the local HTTPS URL from settings into your API client, using the **same API key** saved for that provider. Claude for Office uses `https://localhost:8788/claude` by default.
6. With background startup enabled, close settings; after reboot/sign-in, only the proxy worker starts. With **Start without background startup**, keep the app open while using your clients. Closing or quitting stops the proxy; reopen the app and select **Start proxy** to run it again. There is no Start at login checkbox.

Setup resumes from saved progress if interrupted. **Open settings** provides access to ports and diagnostics; **Resume setup** returns to the wizard. Saving a key alone does not enroll a new installation for background startup or start its listeners. Existing installations retain their settings and skip the wizard. HTTPS remains optional in normal settings for those installations; new onboarding includes it. Developer builds clearly report that they do not register background startup.

Select **Skip setup** at any step to save your entered details and open settings. A typed key is saved in the OS credential store before leaving; an endpoint-only edit is retained without pretending a key is configured. If saving fails, setup stays open so you can retry. Previously saved keys and certificate progress are retained. Skipping stays remembered after reopening, and **Resume setup** lets you return. It does not create/trust a certificate, start a stopped proxy, or newly enable background startup. From settings, save a key and choose **Start proxy** when ready to run with your current configuration and request background startup; HTTPS can be configured later.

With background operation selected, close or quit settings to leave the proxy and active requests running. Open **ICA proxy** from Applications or the Windows Start menu to return to settings. **Stop proxy** stops requests until you select Start or the worker starts again after sign-in. Background startup status appears under **Background & startup**. Foreground users can choose **Enable background startup** there later. macOS may require approval in System Settings → General → Login Items & Extensions; subsequent OS-disabled worker registration is respected.

After onboarding, if macOS reports that background startup needs approval, opening settings shows an **Allow background startup** prompt. Select **Open Login Items settings**, enable ICA proxy under **Allow in the Background**, then return and select **Check background startup**. You can choose **Later** and use the buttons under **Background & startup** when ready. The prompt appears at most once per settings session; sign-in still launches only the worker. macOS controls approval, so the app cannot grant it or force a native Allow dialog.

**Check background startup** also retries a failed registration without reopening the app. If macOS cannot find the service, the app validates its bundled files, attempts registration once, and checks the result, even if it was registered previously. During incomplete onboarding, use the final setup step to register/retry. Checks preserve OS-disabled startup and the reset preference. If the proxy is running, the app asks before checking because handing the worker over to the OS can interrupt requests. Installation, missing-service, and signing failures have separate guidance and safe error codes; a signing failure requires a repaired app build, rather than more permission. If registration still fails, share the displayed error code with the app publisher. On Windows, the existing per-user task needs no administrator prompt; managed-device restrictions may require IT assistance.

On macOS, an enabled background service can still refer to an older app after an update. If the installed worker does not respond, ICA proxy makes one automatic attempt per settings session to refresh an existing enabled registration, then verifies the worker's version and run mode. **Check background startup** can also refresh an enabled registration even when its saved version matches. Native signing/approval errors remain visible. Recovery preserves foreground mode, OS-disabled startup, incomplete setup, and reset preferences; it does not delete provider keys, settings, or certificate trust. A failed startup is not a reason to reset app data.

If macOS reports **SMAppService error 3**, quit ICA proxy and replace the entire app in Applications with a newly verified installer from the publisher. Reopen the Applications copy and retry background startup. Resetting settings or granting background permission cannot repair a damaged signature. Keep saved settings during this repair; do not use **Reset data / prepare to uninstall**. Older builds made without an explicit signing identity can contain only an incomplete linker signature. The source now defaults to full ad-hoc development signing and checks packaged app/worker signatures, but a preview's native background-startup compatibility still requires testing.

When upgrading from llm_proxy to ICA proxy, quit the old app first. On Mac, install ICA proxy in Applications and remove the old llm_proxy app after confirming the new app works. On Windows, uninstall llm_proxy through Installed apps before installing ICA proxy, because the new product name uses a different installation directory. To keep your provider keys, settings, and HTTPS identity, skip **Reset data / prepare to uninstall** during this rename upgrade.

The new worker is bundled into graphical installers. The old GUI startup entry is migrated automatically. The installers include the background worker, Keychain guidance, and combined HTTPS setup button. The existing Word guide describes the previous version.

### Remember Keychain access on macOS

Install the app in **Applications** and run that installed copy. When macOS asks whether **llm-proxy-worker** can access a saved Keychain item, enter your **login Keychain password** (usually your Mac login password) and select **Always Allow**. This lets macOS remember approval for this app; choosing **Allow** or **Allow Once** grants access for the current request. Claude, OpenAI, and the local HTTPS private key are separate items, so approve each applicable prompt. Existing installations may need one new approval for the worker after upgrading; the saved keys and certificate are retained.

Open settings and select **Check saved-key access** after saving your provider keys and setting up HTTPS. It deliberately reads configured secrets so you can handle approval during setup, without sending a provider request, displaying keys, or restarting the proxy. A successful check confirms current access only; the app cannot detect whether you selected **Always Allow**. If startup previously failed, select **Start proxy** after resolving access.

**Always Allow** remembers application access; a locked login Keychain still needs unlocking. If prompts continue after reboot with the same installed app, check that your login Keychain unlocks when you sign in. The current ad-hoc signed builds can require renewed approval after an update changes the executable identity. Stable Developer ID signing is a release follow-up. The app leaves Keychain access control to macOS and never asks for your Keychain password inside its own settings.

## Client connection

The default local port is `8787`, configurable while the proxy is stopped.

For Claude for Office, set up HTTPS as described below and use `https://localhost:8788/claude`. New onboarding includes HTTPS; it remains optional in normal settings and uses a separate port so existing HTTP clients can keep their current URLs.

| Client protocol    | Local base URL                    | Upstream default                            |
| ------------------ | --------------------------------- | ------------------------------------------- |
| OpenAI             | `http://127.0.0.1:8787/openai/v1` | `https://api.servicesessentials.ibm.com/v1` |
| Claude / Anthropic | `http://127.0.0.1:8787/claude`    | `https://api.servicesessentials.ibm.com`    |

For example, an OpenAI client sends requests to `/openai/v1/chat/completions` or `/openai/v1/responses`; an Anthropic client sends requests to `/claude/v1/messages`. Clients must support setting their API base URL. Model names and bodies pass through without translation. An OpenAI client cannot use Claude's native protocol through this app.

Authenticate locally with `Authorization: Bearer <provider-api-key>` or `x-api-key: <provider-api-key>`, using the same API key saved for that provider in the app. Anthropic clients must also send their usual `anthropic-version` header. The proxy validates the key against the selected provider and sends it upstream using that provider's native authentication header. There is no separate local token. When you replace a provider key, update your clients too; existing streams can finish while new requests require the replacement key.

Streaming responses are forwarded incrementally. Upstream errors and rate-limit headers pass through, and generation requests are never automatically retried. Remote upstreams require validated HTTPS; HTTP is accepted only for explicitly configured loopback IP addresses. Listeners bind only to IPv4/IPv6 loopback. Browser-origin calls are restricted as described below; arbitrary forward-proxy traffic is rejected.

Default limits: 8 active requests, 32 KiB request headers, 32 MiB uploaded bodies, a 10-second upstream connection timeout, and 120-second response-header/stream-idle timeouts. Limits apply to transport resources, not model output token counts. Local capacity exhaustion returns HTTP 429; a stream interrupted after response headers closes without an invented provider event.

### Claude for Office browser access

The proxy allows the exact browser origin `https://pivot.claude.ai` by default, which Anthropic documents for Office gateway connections. Valid preflight OPTIONS requests are answered locally without a key; actual GET/POST requests require the same saved provider API key as native clients. Requested header names are validated and echoed, allowing new SDK headers without broadening the trusted origin. CORS headers cover successes, errors, and incremental streams. Unlisted origins, including other `claude.ai` subdomains by default, are rejected. Browser cookies are not used for authentication.

Office's gateway URL must use HTTPS. To enable the app's local HTTPS listener without any terminal commands:

1. Stop the proxy and open **Office gateway HTTPS** in settings.
2. Keep HTTPS port `8788` or apply a different unused port. It must differ from the HTTP port.
3. Select **Set up HTTPS and trust certificate** and approve any OS authorization prompt. This single action creates a certificate unique to this device, keeps its private key in the OS credential store, adds only its own public root to current-user trust, and verifies trust. Managed devices may require administrator approval. If approval fails, use the same button to retry; the saved certificate is reused.
4. Fully quit and reopen PowerPoint/Excel, start the proxy, and copy **Claude HTTPS** into Office: `https://localhost:8788/claude`. Use the same Claude API key saved in the proxy.

The OpenAI HTTPS URL is `https://localhost:8788/openai/v1`. The certificate also covers `127.0.0.1` and `::1`. HTTPS has the same CORS rules, authentication, streaming, and request budget as HTTP. Merely changing the scheme on the HTTP port does not enable TLS; use the displayed HTTPS URL and port.

The certificate lasts one year; its expiration and root fingerprint are shown in settings. **Renew HTTPS and trust certificate** confirms replacement while stopped, removes old trust, and requests trust for the replacement in the same action. Restart Office afterward. **Remove HTTPS** removes this identity's trust, public metadata, and private key. **Check certificate trust** refreshes status after an OS trust change. Certificate setup requires an explicit action in onboarding or settings and is saved for later starts, including login startup; trust is never silently installed in the background. No private certificate key is stored in the repository or shared with the developer Rust bundle.

The installers include the combined setup button. The older Word guide still shows separate setup and trust buttons.

Local tests verify certificate validation, HTTP/2, CORS, authentication, incremental HTTPS streams, and shutdown. Native trust prompts and an end-to-end Office connection still need platform acceptance testing; certificate trust, local-network permissions, and enterprise policies can vary by Office/OS installation.

To add a future documented Office origin, stop the proxy, expand **Trusted browser origins**, enter one exact HTTPS origin per line, select **Save browser origins**, then start the proxy. Keep the existing Office origin if it is still in use. Up to 16 unique origins are supported; omit trailing slashes, paths, credentials, and wildcards. An empty list disables browser access. Existing settings automatically receive the default Office origin when upgrading.

The origin validation is centralized in [cors.rs](crates/proxy-core/src/cors.rs). Adding exact origins through the GUI avoids waiting for an app release. Automatically trusting all subdomains expands browser trust to every current and future host under that domain; API-key authentication would still be required, and that broader policy cannot guarantee future protocol or domain compatibility. See [Anthropic's gateway documentation](https://support.claude.com/en/articles/13945233-use-claude-in-excel-and-powerpoint-with-an-llm-gateway).

## Privacy and removal

Provider keys and the optional HTTPS server private key use macOS Keychain or Windows Credential Manager. Versioned JSON settings contain only endpoints, HTTP/HTTPS ports, HTTPS enablement, a legacy startup compatibility field, onboarding completion, and trusted browser origins. A separate JSON file contains only the public certificate chain and expiration. Another non-secret file records worker registration/reset state and its executable fingerprint. The issuing private key is discarded after certificate generation. Diagnostics are a 64-entry memory ring with lifecycle/error messages; prompts, responses, authorization headers, and full request URLs are not recorded. There is no telemetry, hosted relay, or account service.

Settings uses a private current-user Unix socket on macOS or restricted named pipe on Windows. Revealed keys cross that IPC and Tauri IPC only after a deliberate action. Other native processes running as the same OS user are within the management channel trust boundary. No management HTTP port or new token is created. Owned Rust secret buffers are zeroized where practical; this cannot erase every temporary copy made by the GUI, OS, or transport libraries.

Before uninstalling, select **Reset data / prepare to uninstall**, confirm, then quit. This stops the proxy, disables startup, removes the app's HTTPS certificate trust and private key, deletes stored provider credentials (including any obsolete token from earlier builds), and resets settings. Allow any OS certificate-removal prompt; if cleanup is denied, resolve the permission and retry. On macOS, remove the app through Finder. On Windows, use Installed apps; the uninstaller also stops the worker and removes its scheduled task and old startup registry entries. Removing application files alone can leave native trust and credential-store entries, so use the reset action first. Empty/default settings and a disabled registration marker may remain in the OS app-config directory. Opening settings after reset does not silently enroll startup again; the final onboarding action explicitly enrolls startup again.

## Develop

Builds automatically compile and bundle the worker when using `npm run tauri build` or `npm run tauri dev`. Before running workspace Cargo checks directly, stage it once with `npm run worker:build`. Build-only tools are never required by end users.

Developer prerequisites: Node.js 22.18+, npm, Rust via rustup (the exact version is in `rust-toolchain.toml`), and native Tauri build prerequisites. macOS needs Apple's command line build tools. Windows needs the Visual Studio C++ desktop build tools, Windows SDK, and WebView2. These are development requirements only.

### Repository-local Rust

The working Apple Silicon macOS Rust installation has been moved into this checkout:

```text
.toolchains/rust/
  cargo/                  # rustup launcher, Cargo shims, and cached dependencies
  rustup/                 # installed Rust compiler, standard library, and components
```

`npm run tauri` and `npm run cargo` automatically use this bundle when present. The wrapper derives all paths from the repository location, sets `CARGO_HOME`/`RUSTUP_HOME`/`RUSTUP_TOOLCHAIN` only for the child process, and checks the compiler's version against `rust-toolchain.toml`. It does not require shell profile changes. Without the bundle, these commands use the system Rust installation, including in CI.

To share the toolchain, copy the **entire `.toolchains/rust/` folder**, including hidden files, executable permissions, and symlinks, into the recipient's repository at the same relative path. On macOS you can create an archive from the repository root:

```sh
tar -czf llm-proxy-rust-macos-arm64.tar.gz .toolchains/rust
```

Extract it from the recipient's repository root:

```sh
tar -xzf /path/to/llm-proxy-rust-macos-arm64.tar.gz
npm ci
npm run cargo -- --version
npm run tauri build -- --bundles app,dmg
```

The current bundle is **Rust 1.100.0 for Apple Silicon macOS only**. Intel Macs and Windows require separate matching host bundles or their own Rust installation; this copy does not provide cross-platform compilation. Native Tauri build prerequisites are still required on each host. The Cargo cache can reduce downloads but does not guarantee an offline build. Use the wrapper instead of calling the copied binaries directly so they receive the correct environment.

`.toolchains/` is excluded from Git and formatting because it contains large platform-specific binaries and caches. Transfer it separately; a Git clone will not include it. Generated archives named `llm-proxy-rust-*.tar.gz` are also ignored by Git. For rustup's supported environment variables and host-specific toolchains, see the [rustup documentation](https://rust-lang.github.io/rustup/environment-variables.html).

### Development commands

```sh
npm ci
npm run tauri dev
```

`npm run dev` alone opens only a read-only browser preview; running a proxy and saving keys require the native app. The Rust proxy core has no desktop dependency and can be tested independently.

```sh
npm run check
npm run lint
npm run format:check
npm test
npm run cargo -- fmt --all -- --check
npm run cargo -- clippy --workspace --all-targets --locked -- -D warnings
npm run cargo -- test --workspace --locked
```

Build native packages on their respective architectures:

```sh
# macOS: development .app and .dmg; complete ad-hoc signing is the default
npm run tauri build -- --bundles app,dmg

# Windows: graphical setup .exe
npm run tauri build -- --bundles nsis
```

Artifacts are written under `target/release/bundle/`. The [CI workflow](.github/workflows/ci.yml) runs checks and creates development packages for both Mac architectures and Windows x64, using ad-hoc signing on Mac and no publisher signature on Windows. It does not publish releases. Production releases require protected signing/notarization; preview downloads remain explicitly labeled as development packages.

### macOS signing and installer verification

`bundle.macOS.signingIdentity` defaults to `-`. This makes Tauri sign the embedded worker and seal the complete app bundle after assembling its resources, even when `APPLE_SIGNING_IDENTITY` is unset. Do not replace executables or resources inside the app afterward. This repairs the incomplete-linker-signature packaging defect; ad-hoc signing does not establish a trusted publisher or prove that macOS will accept native background-service registration.

Successful macOS `npm run tauri build` and `npm run tauri bundle` commands automatically run [the package verifier](scripts/verify-macos.mjs). It checks the bundle resource seal, strict/deep app signature, independent worker signature, matching signing teams, and LaunchAgent target. It also verifies DMG checksums, mounts images read-only without opening Finder or launching the app, and checks that the packaged app and worker signatures match the build. A mismatch fails the command. Clear obsolete installers from the build output before bundling a new revision: an old DMG that contains a different app will deliberately fail verification. Compile-only `--no-bundle` builds skip package verification; `--no-sign` packages are unsuitable for background-startup acceptance.

Run these checks separately on macOS:

```sh
npm run test:packaging
npm run verify:macos -- --require-dmg
```

For an explicit Rust target, pass its output directory, for example `npm run verify:macos -- target/aarch64-apple-darwin/release/bundle --require-dmg`.

For production distribution, install a **Developer ID Application** certificate and its private key in the protected build environment. Set `APPLE_SIGNING_IDENTITY` to that identity, overriding the development default. Configure Tauri notarization with `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password), and `APPLE_TEAM_ID`, or the supported App Store Connect API credentials. Keep credentials in CI secrets, never in the repository. See [Tauri's macOS signing documentation](https://v2.tauri.app/distribute/sign/macos/).

After the signed/notarized build, require Developer ID identities on both the app and worker and separately validate notarization/Gatekeeper acceptance:

```sh
npm run verify:macos -- --require-dmg --require-developer-id
xcrun stapler validate "target/release/bundle/macos/ICA proxy.app"
spctl --assess --type execute --verbose=2 "target/release/bundle/macos/ICA proxy.app"
```

The verifier never repairs or re-signs an app. Notarization and signature integrity are separate checks; neither replaces installing the actual DMG on a clean account and verifying onboarding, native background approval/registration, and sign-in startup on each supported macOS architecture.

## Local provider simulation

The normal Rust tests start isolated localhost upstreams, require no internet or real keys, and cover routing, credentials, SSE, cancellation, limits, failures, and shutdown. A separate locally authored Postman simulation exercises the same production proxy core against two service stand-ins.

With the Postman CLI installed, run these in separate terminals:

```sh
postman simulation run postman/simulations/native-providers.sim.yaml --port 4010 --no-history --output ndjson
npm run cargo -- run -p proxy-core --example fixture_proxy --locked
node scripts/verify-simulation.mjs
```

The example uses fixed, unmistakably fake credentials, listens on port `4011`, and never touches desktop settings or the OS credential store. Stop both servers with Ctrl+C. See the [simulation evidence and assumptions](postman/simulations/native-providers.context.yaml). Its passing checks establish local forwarding behavior; live provider compatibility remains a separate check.

## Validation and release gates

Verified locally on Apple Silicon macOS: Rust compilation/lint, deterministic proxy and settings tests, frontend types/lint/tests/build, and settings preview. Native packaging and lifecycle observations are recorded as they are completed in [validation notes](VALIDATION.md).

Before a production release, complete Intel Mac and Windows execution, real-provider/client smoke checks, signed/notarized installer tests, credential-store failure recovery, clean-account install/uninstall, and reboot/sign-in startup tests. The required **combined background RAM below 200 MB**, ideally below 100 MB, is a release gate, not an unmeasured framework promise. The full idle/8-stream/open-close workload and helper-process attribution are defined in [Architecture §11](ARCHITECTURE.md#11-validation-and-implementation-sequence).
