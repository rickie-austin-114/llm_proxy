# ICA proxy Requirements

Status: MVP requirements; implementation present, release acceptance pending

Date: November 19, 2026

## 1. Purpose

Build a lightweight desktop application that runs a local proxy for the Claude and OpenAI APIs. Users enter their API key and upstream endpoint in a GUI, start the proxy, and connect their existing API clients to a localhost address.

Installation, setup, and everyday operation must be possible without a terminal or command-line tools.

## 2. User requirements

- Use **ICA proxy** as the app name in the GUI, native menus, and installers.
- Distribute the application as open-source software under the MIT license, with source and license available in the repository.
- Support MacBooks with Intel processors and Apple Silicon processors.
- Support Windows computers.
- Provide a lightweight application with a graphical setup experience.
- Let users configure Claude and OpenAI API keys and upstream endpoints.
- Expose a proxy on localhost.
- Make installation straightforward and entirely graphical.
- Automatically start a headless proxy worker after reboot/sign-in; keep the settings application closed.
- Keep background RAM usage strictly below 200 MB, ideally below 100 MB, across all application processes.

The remaining sections define the proposed minimum viable product (MVP). Items explicitly labeled as proposed defaults or targets can be adjusted before implementation.

## 3. Supported platforms and installation

| Platform | Required architecture | Installation experience |
| --- | --- | --- |
| macOS | Intel, x86_64 | Download and open a disk image or installer; install through the GUI. |
| macOS | Apple Silicon, arm64 | Download and open a native or universal application; install through the GUI. |
| Windows | x64 | Download and run a graphical installer. |

- Supply either a universal macOS application or clearly labeled downloads for both Mac architectures.
- The Apple Silicon build must run natively without requiring Rosetta.
- Bundle application dependencies or install any required runtime automatically through the graphical installer.
- Do not require users to install Python, Node.js, Docker, a package manager, or development tools.
- Do not require users to run commands, edit configuration files, or set environment variables to use the application.
- Prefer installation and operation without administrator privileges. Document any unavoidable OS permission prompt.
- Public releases must be signed on both platforms and notarized on macOS. Users must not need to disable OS security protections.
- Provide a graphical uninstall path and a way to remove stored credentials and settings.

Initial baselines are macOS 13+ and Windows 11 x64. Windows on ARM is outside the initial commitment unless added explicitly. Validate each baseline before release.

## 4. First-run experience

1. Opening a new installation or resetting app data presents a three-step onboarding flow.
2. **Provider key:** choose either Claude or OpenAI and save one API key. Editable endpoint defaults are `https://api.servicesessentials.ibm.com` for Claude and `https://api.servicesessentials.ibm.com/v1` for OpenAI. The second provider and optional non-generation connection test remain available in settings.
3. **Trusted HTTPS:** a single **Set up HTTPS and trust certificate** action creates and trusts the localhost identity through the existing native OS flow. Verify enabled HTTPS, certificate validity, and current trust before advancing. Retain OS approval/password prompts; retry canceled trust against the saved valid certificate.
4. **Background proxy:** explicitly start the proxy and enroll background startup. Show macOS permission guidance and direct navigation to Login Items settings when required, then allow a deliberate approval check/retry. Finish only after the HTTPS proxy is running with an available provider key and background startup is enabled. Development builds clearly disclose that OS startup registration is omitted.
5. Display running status and copyable local URLs. Clients use the same API key saved for the selected provider. Closing settings preserves the worker; subsequent sign-in launches the worker without opening the GUI.

Persist completion and derive the current step from authoritative provider/certificate state, so interruption or denied approval can be resumed. Never persist the entered key in frontend storage. Do not automatically start proxy listeners or newly enroll background startup before the final step. Keep settings accessible for diagnostics/port conflicts, with a way to resume onboarding. Existing installations skip onboarding and retain their settings, HTTPS choices, and OS startup opt-outs. Reset starts onboarding again.

All application steps must be available in the GUI. Distinguish local readiness from an upstream connection test; completing onboarding does not claim the provider accepts the key or that Office has connected.

Offer **Skip setup** on every onboarding step. Before leaving, securely save a newly typed provider key and its endpoint, or preserve an endpoint-only edit as non-secret draft configuration without marking that provider as configured. Keep existing provider/certificate progress. If validation, credential storage, or settings persistence fails, leave setup open with unsaved input available for correction/retry. Remember dismissal across launches and offer **Resume setup**. Skipping alone must not create/trust certificates, start stopped listeners, or newly enroll startup. Users may instead save a provider in settings and explicitly **Start proxy**, using the current HTTP/HTTPS configuration and requesting background startup without requiring the guided HTTPS flow. A successful manual start accepts this settings-based configuration for future worker launches. Reset clears the skip preference and drafts and restores onboarding.

## 5. GUI requirements

- Provide separately saved configurations for Claude and OpenAI, with both usable at the same time.
- Each provider configuration includes an API key and an editable upstream base URL.
- Mask API keys by default and provide deliberate reveal, replace, and delete controls.
- Provide a configurable local port with a sensible default.
- Provide **Test Connection**, **Start Proxy**, and **Stop Proxy** actions.
- Show whether each provider is configured and whether the local server is stopped, starting, running, or in an error state.
- Display and copy the exact local base URL for each provider, including any required path prefix.
- Explain which API protocol clients should use for each local endpoint.
- Explain errors in plain language and suggest a useful next action.
- Preserve settings and credentials between launches using appropriate storage.
- Support keyboard navigation, readable text, and status indicators that do not rely on color alone.
- Show automatic background startup status and actionable OS permission guidance; remove the Start at login checkbox.
- Keep Start Proxy / Stop Proxy controls in settings; reopen settings from Applications or the Windows Start menu.

The headless worker runs independently of the settings app, with no resident GUI, tray, or webview dependency. Closing or quitting settings exits the GUI and leaves existing proxy requests running. Register a new worker during the final onboarding step and migrate the old GUI startup entry after enrollment. On subsequent launches, preserve OS-disabled worker registration and show that state. OS approval prompts remain under user control.

On macOS, validate the app's bundled service files/configuration before registration. A service-not-found status must allow one deliberate registration attempt from the final onboarding action or **Check background startup** after onboarding, including when a previous enrollment marker exists. Ordinary inspection must not repeatedly attempt repair. Verify enabled/pending-approval status before saving enrollment; preserve useful native errors if repair fails. A check must not undo reset, bypass incomplete onboarding, or reenable existing OS-disabled enrollment.

Startup occurs after user sign-in, using native per-user credentials/trust. Running before sign-in is outside scope. Locked credentials produce actionable diagnostics without plaintext fallback or repeated prompts. Reset/pre-uninstall removes credentials/trust and worker registration, stops the worker, and leaves automatic startup disabled until an explicit new setup. Stop Proxy closes listeners until Start or a subsequent worker launch/sign-in. Uninstall must stop the worker and remove registration.

## 6. Proxy requirements

### 6.1 Provider behavior

- Support native Claude API requests and native OpenAI API requests through distinct, clearly identified local endpoints.
- Forward each request to the configured provider endpoint and authenticate it with that provider's stored API key.
- Preserve the native request and response formats, HTTP methods, relevant headers, query parameters, and upstream status codes.
- Preserve upstream base paths when constructing destination URLs.
- Support streaming and non-streaming responses. Deliver streamed events incrementally without buffering the full response.
- Allow concurrent requests and release upstream resources when a client disconnects or cancels a request.
- Return useful errors for invalid credentials, unavailable endpoints, rate limits, timeouts, and upstream failures without exposing secrets.
- Do not automatically retry requests that could duplicate generation or billing.
- Do not hard-code a fixed list of model names; pass model identifiers through to the selected provider.

MVP assumption: the proxy preserves each provider's protocol. Translating OpenAI-format requests into Claude-format requests, or the reverse, is a separate feature and is not required for the first version.

### 6.2 Local server behavior

- Bind only to loopback interfaces. Remote devices must not be able to connect.
- Keep localhost HTTP available for compatible native clients; new-user onboarding also sets up trusted HTTPS for Office. Preserve HTTP-only configurations on upgrade.
- Provide localhost HTTPS for Claude for Office, included in new-user onboarding and optional for existing installations, on a separate configurable port, default 8788. Generate a unique device certificate, store its private key in the OS credential store, and provide graphical trust, trust checking, renewal, and removal without terminal commands. A single **Set up HTTPS and trust certificate** action creates the identity, requests current-user trust, and verifies it; retain required OS approval prompts. Retry denied trust using the saved certificate rather than generating a replacement. Renewal also includes trust after confirming replacement. Preserve existing HTTP client URLs. Reset/pre-uninstall removes the app's HTTPS trust and private key.
- Handle IPv4 and IPv6 loopback consistently where supported, and display an address that is actually reachable.
- Detect port conflicts and allow users to choose another port through the GUI.
- Keep provider routing unambiguous when both providers are configured.
- Stop accepting new requests when stopped or when the worker exits, and close active requests cleanly with a bounded shutdown time.
- Prevent multiple app instances from silently starting conflicting proxy servers.

## 7. Credentials, privacy, and security

- Store upstream API keys in the OS credential store: macOS Keychain and a suitable Windows credential facility.
- On macOS, guide users to choose **Always Allow** in the native Keychain prompt for remembered application access. Provide a user-initiated check of configured provider keys and the optional HTTPS private key without exposing secrets or sending network requests. Explain that locked Keychains and changed app identities can still require approval; never collect the Keychain password in the app or grant access to all applications.
- Do not store API keys in plaintext configuration files, logs, crash reports, or source code.
- Use HTTPS for remote upstream endpoints and validate certificates. Permit HTTP only for explicitly configured loopback upstream endpoints.
- Forward credentials only to the selected, saved upstream origin. Do not follow redirects that would disclose credentials to another origin.
- Do not generate a separate local access token. Local clients use the API key entered and saved by the user for the selected provider.
- Validate the supplied Bearer or `x-api-key` credential against that provider's saved API key. Reject missing, incorrect, duplicate, or conflicting credentials and keys belonging only to another provider. Apply the selected provider's native upstream authentication after validation.
- Reject unauthenticated API requests. Allow unauthenticated CORS preflights only after validating the browser origin, local Host, provider namespace, method, and header limits; never forward preflights upstream.
- Default browser access to the exact HTTPS origin `https://pivot.claude.ai` for Claude for Office. Provide a GUI list of up to 16 unique exact HTTPS origins, editable while stopped, so users can approve documented future Office origins without an app update. An empty list disables browser access. Reject unlisted origins, lookalikes, wildcards, URL paths, credentials, and duplicate Origin headers. Actual requests still require the selected provider's saved API key.
- Handle GET/POST/OPTIONS browser access, echo validated preflight header names for SDK compatibility, and include local CORS headers on successful and error responses. Upstream CORS must not expand this policy. Do not enable unrestricted browser access or cookie-based browser credentials.
- Do not log prompt content, response content, or authorization headers by default.
- Keep operational logs minimal, redact sensitive values, and bound log storage.
- Route API traffic directly from the user's computer to the configured upstream endpoint. Do not require an application account or a hosted relay service.
- Do not collect telemetry by default. Any future telemetry must require explicit opt-in.
- Deleting a provider configuration must remove its stored credential.

## 8. Lightweight operation and reliability

Choose the implementation stack based on cross-platform support, packaged size, memory use, startup time, and maintainability. `ARCHITECTURE.md` selects Tauri 2 + Rust with Svelte/TypeScript, subject to the resource acceptance gate.

The background memory limit is a required budget. Other performance figures below are proposed targets. Validate all measurements against representative machines on every supported architecture.

| Measure | Initial target |
| --- | --- |
| Installer download size | At most 75 MB per architecture. |
| Background memory usage | Strictly below 200 MB total across all application processes, including helpers; ideally below 100 MB. |
| Idle CPU usage | Below 1% on average over 60 seconds after startup. |
| Time to usable GUI | At most 3 seconds on a typical supported machine. |
| Proxy forwarding overhead | At most 50 ms added median time to first response byte in a controlled local upstream test. |

- Keep the GUI responsive while requests are running or streaming.
- Keep automatic startup and background operation lightweight. Release unnecessary GUI resources when the setup window is closed and avoid frequent polling or wakeups while idle.
- Verify the background RAM budget after startup has settled, with the proxy enabled and the setup window closed, both while idle and during a defined representative forwarding workload. Record the peak combined resident memory of the app and its helper processes during each measurement; the below-100-MB figure is an optimization target, while below 200 MB is mandatory.
- Do not load or run language models locally.
- Handle network loss and upstream outages without crashing or losing saved configuration.
- Bound request buffering and resource consumption so slow clients or large responses do not cause uncontrolled memory growth.
- Provide GUI access to redacted diagnostic information for troubleshooting.
- Setup and local server startup must work without internet access; upstream requests and connection tests require access to the configured endpoint.

## 9. MVP acceptance criteria

1. A user can install and complete setup through the GUI on an Intel Mac, an Apple Silicon Mac, and a supported Windows x64 computer without running commands.
2. The user can save, test, replace, and delete credentials and endpoints for both providers, and saved configurations survive an app restart.
3. A client using the native Claude protocol can complete both a streaming and a non-streaming request through the local Claude endpoint.
4. A client using the native OpenAI protocol can complete both a streaming and a non-streaming request through the local OpenAI endpoint.
5. Both providers can serve concurrent requests without mixing destinations or credentials.
6. An invalid key, unreachable endpoint, occupied port, upstream rate limit, and interrupted stream each produce an understandable error without crashing the app.
7. The local server accepts loopback requests authenticated with the selected provider's saved API key, rejects requests without that key, and cannot be reached from another device on the network. Replacing a provider key updates local authentication for new requests without interrupting an existing stream.
8. Stored configuration and diagnostic output contain no plaintext API keys or default prompt/response logging.
9. Stop Proxy releases the listening port; closing or quitting settings preserves proxy availability and active requests.
10. Release artifacts and runtime measurements are checked on all supported architectures against the agreed installation and lightweight-operation requirements.
11. Reboot/sign-in launches only the registered worker and makes the configured proxy available without a settings window. OS-disabled worker startup is respected on subsequent launches.
    When macOS requires background-item approval after onboarding, opening settings prompts once per session and offers direct navigation to Login Items settings. Users can defer and later check/retry from the GUI. Permission, installation, and signing failures must have distinct guidance; the app must not bypass OS approval or display setup prompts from the headless worker.
12. Background operation remains below 200 MB combined RAM across app and helper processes under the defined idle and representative forwarding checks; results also report progress toward the ideal of below 100 MB. Repeatedly opening and closing the GUI must not cause sustained memory growth.
13. New installations complete the three-step provider → trusted HTTPS → running background proxy flow, resume after cancellation/relaunch, and do not start automatically during partial setup. Existing installations bypass the wizard.
14. Closing/quitting settings exits its processes while preserving the worker. Reset/pre-uninstall removes startup registration and stops the worker; graphical installation bundles both executables.

## 10. Outside the initial scope

- Translation between Claude and OpenAI API formats.
- Other providers, cloud-specific provider variants, or local model hosting.
- LAN/public hosting, multi-user administration, billing, quotas, or team accounts.
- Request caching, automatic provider failover, and model routing rules.
- A built-in chat interface or developer console.
- A required CLI, browser extension, or hosted backend.

## 11. Decisions and release validation

The MVP uses macOS 13+ and Windows 11 x64, Tauri 2/Rust with Svelte/TypeScript, port 8787, and distinct native provider prefixes as documented in `ARCHITECTURE.md` and `README.md`. Protocol translation remains outside the initial scope. Native Mac disk images and a per-user Windows installer provide graphical installation.

Before release, finalize and verify:

- Client applications and optional API surfaces covered by compatibility testing.
- Reference machines for the memory workload defined in `ARCHITECTURE.md`, plus final values for other proposed performance targets.
- Signing, notarization, download distribution, and the manual update process.
- Clean-machine installation, native credential handling, startup, and uninstall behavior on every supported architecture.
