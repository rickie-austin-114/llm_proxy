# llm_proxy

A small desktop app that keeps your Claude and OpenAI keys in the OS credential store and forwards native API requests through localhost. Built with Tauri 2, Rust, and Svelte.

## Project documents

- [Requirements](REQUIREMENTS.md): supported platforms, GUI setup, security, startup, and resource budgets.
- [Architecture](ARCHITECTURE.md): framework decisions, API routing, lifecycle, and release validation.
- [Coding style](CODING_STYLE.md): implementation conventions, component boundaries, and checks.

## Install and configure

See the [Word installation guide](docs/LLM_Proxy_PowerPoint_Installation_Guide.doc) for graphical installation, local HTTPS certificate setup, and Claude in PowerPoint instructions with screenshots.

Download the **v0.1.0 preview** installer for your computer:

| Computer                 | Download                                                                                                                            |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| Mac with Apple Silicon   | [Download DMG](https://github.com/rickie-austin-114/llm_proxy/releases/download/v0.1.0/LLM-Proxy-0.1.0-macOS-Apple-Silicon.dmg)     |
| Mac with Intel processor | [Download DMG](https://github.com/rickie-austin-114/llm_proxy/releases/download/v0.1.0/LLM-Proxy-0.1.0-macOS-Intel.dmg)             |
| Windows x64              | [Download setup EXE](https://github.com/rickie-austin-114/llm_proxy/releases/download/v0.1.0/LLM-Proxy-0.1.0-Windows-x64-Setup.exe) |

These are development packages: the Mac apps have an ad-hoc signature without Apple notarization, and the Windows installer has no publisher signature. Signed production releases and the platform acceptance checks below are still pending. See the [release notes](https://github.com/rickie-austin-114/llm_proxy/releases/tag/v0.1.0) for limitations and checksums.

The intended graphical installation is:

- **macOS 13 or newer:** open the disk image for your processor, then drag **llm_proxy** into Applications. Separate native builds cover Intel and Apple Silicon.
- **Windows 11 x64:** run the setup executable. It installs for the current user and bootstraps WebView2 if missing; the runtime download requires internet access.

End users do not need Node.js, Rust, a package manager, or a terminal.

1. Open **llm_proxy** and enter a Claude or OpenAI API key. Both providers can be configured together.
2. Keep the default upstream URL or enter your compatible gateway's base URL. OpenAI URLs include the API prefix (normally `/v1`); Claude URLs use the service root.
3. Optionally select **Test connection**. This lists models; it does not generate tokens or save your input.
4. Select **Save settings**, then **Start proxy**.
5. Copy the appropriate local base URL into your client. Use the **same API key** you saved for that provider in the app.
6. **Start at login** is enabled by default on first launch. After signing in following a reboot, the app starts the saved proxy in the background. Disable it in settings if you prefer to launch manually.

Closing settings releases the window while the menu bar/system tray remains available. Use its menu to reopen settings, start/stop the proxy, or quit. Quitting stops requests and exits. Automatic startup occurs at user sign-in. Existing saved startup preferences are preserved.

## Client connection

The default local port is `8787`, configurable while the proxy is stopped.

For Claude for Office, set up HTTPS as described below and use `https://localhost:8788/claude`. HTTPS is optional and uses a separate port so existing HTTP clients can keep their current URLs.

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
3. Select **Set up HTTPS**. The app creates a certificate unique to this device and keeps its private key in the OS credential store.
4. Select **Trust certificate**, confirm, and approve any OS authorization prompt. The app adds only its own public root to current-user trust, then checks certificate trust. Managed devices may require administrator approval.
5. Fully quit and reopen PowerPoint/Excel, start the proxy, and copy **Claude HTTPS** into Office: `https://localhost:8788/claude`. Use the same Claude API key saved in the proxy.

The OpenAI HTTPS URL is `https://localhost:8788/openai/v1`. The certificate also covers `127.0.0.1` and `::1`. HTTPS has the same CORS rules, authentication, streaming, and request budget as HTTP. Merely changing the scheme on the HTTP port does not enable TLS; use the displayed HTTPS URL and port.

The certificate lasts one year; its expiration and root fingerprint are shown in settings. **Renew certificate** replaces it while stopped, removes old trust, and requires trusting the replacement and restarting Office. **Remove HTTPS** removes this identity's trust, public metadata, and private key. **Check certificate trust** refreshes status after an OS trust change. Certificate setup is opt-in and saved for later starts, including login startup; trust is never silently installed in the background. No private certificate key is stored in the repository or shared with the developer Rust bundle.

Local tests verify certificate validation, HTTP/2, CORS, authentication, incremental HTTPS streams, and shutdown. Native trust prompts and an end-to-end Office connection still need platform acceptance testing; certificate trust, local-network permissions, and enterprise policies can vary by Office/OS installation.

To add a future documented Office origin, stop the proxy, expand **Trusted browser origins**, enter one exact HTTPS origin per line, select **Save browser origins**, then start the proxy. Keep the existing Office origin if it is still in use. Up to 16 unique origins are supported; omit trailing slashes, paths, credentials, and wildcards. An empty list disables browser access. Existing settings automatically receive the default Office origin when upgrading.

The origin validation is centralized in [cors.rs](crates/proxy-core/src/cors.rs). Adding exact origins through the GUI avoids waiting for an app release. Automatically trusting all subdomains expands browser trust to every current and future host under that domain; API-key authentication would still be required, and that broader policy cannot guarantee future protocol or domain compatibility. See [Anthropic's gateway documentation](https://support.claude.com/en/articles/13945233-use-claude-in-excel-and-powerpoint-with-an-llm-gateway).

## Privacy and removal

Provider keys and the optional HTTPS server private key use macOS Keychain or Windows Credential Manager. Versioned JSON settings contain only endpoints, HTTP/HTTPS ports, HTTPS enablement, startup preference, and trusted browser origins. A separate JSON file contains only the public certificate chain and expiration. The issuing private key is discarded after certificate generation. Diagnostics are a 64-entry memory ring with lifecycle/error messages; prompts, responses, authorization headers, and full request URLs are not recorded. There is no telemetry, hosted relay, or account service.

Revealed keys cross local IPC into the settings window. Owned Rust secret buffers are zeroized where practical; this cannot erase every temporary copy made by the GUI, OS, or transport libraries.

Before uninstalling, select **Reset data / prepare to uninstall**, confirm, then quit. This stops the proxy, disables startup, removes the app's HTTPS certificate trust and private key, deletes stored provider credentials (including any obsolete token from earlier builds), and resets settings. Allow any OS certificate-removal prompt; if cleanup is denied, resolve the permission and retry. On macOS, remove the app through Finder. On Windows, use Installed apps; the uninstaller also removes startup registry entries. Removing application files alone can leave native trust and credential-store entries, so use the reset action first. An empty/default settings file may remain in the OS app-config directory.

## Develop

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
# macOS: development .app and .dmg with an ad-hoc bundle signature
APPLE_SIGNING_IDENTITY=- npm run tauri build -- --bundles app,dmg

# Windows: graphical setup .exe
npm run tauri build -- --bundles nsis
```

Artifacts are written under `target/release/bundle/`. The [CI workflow](.github/workflows/ci.yml) runs checks and creates unsigned test packages for both Mac architectures and Windows x64. It does not publish releases. Protected release signing/notarization must be configured before distributing public installers.

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

Before a public release, complete Intel Mac and Windows execution, real-provider/client smoke checks, signed/notarized installer tests, credential-store failure recovery, clean-account install/uninstall, and reboot/sign-in startup tests. The required **combined background RAM below 200 MB**, ideally below 100 MB, is a release gate, not an unmeasured framework promise. The full idle/8-stream/open-close workload and helper-process attribution are defined in [Architecture §11](ARCHITECTURE.md#11-validation-and-implementation-sequence).
