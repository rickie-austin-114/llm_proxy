# ICA proxy Coding Style

Status: Project conventions for implementation

Date: November 16, 2026

## 1. Scope and principles

Apply these conventions to application code, tests, build configuration, and scripts. Follow [REQUIREMENTS.md](REQUIREMENTS.md) for product behavior and [ARCHITECTURE.md](ARCHITECTURE.md) for component boundaries. This guide defines how to write and review the implementation; it does not change those documents.

- Prefer straightforward code with explicit inputs, outputs, ownership, and failure paths.
- Keep each module focused on one responsibility. Extract helpers when they clarify behavior or remove meaningful duplication.
- Introduce abstractions for real boundaries, such as credential storage or desktop integration. Avoid frameworks and generic wrappers for hypothetical future features.
- Make resource ownership, cancellation, and cleanup visible in code.
- Treat credentials, streaming correctness, and the background RAM budget as implementation constraints.
- Keep changes focused. Separate unrelated refactors and bulk formatting from behavior changes.

## 2. Formatting and file conventions

| Area | Convention |
| --- | --- |
| Text files | UTF-8, LF line endings, final newline, no trailing whitespace. |
| Rust | `rustfmt`, four-space indentation, default 100-column width. |
| TypeScript, Svelte, CSS, JSON | Prettier, two-space indentation, 100-column preferred width. |
| TypeScript strings | Single quotes; use template strings for interpolation. |
| TypeScript punctuation | Semicolons and formatter-managed trailing commas. |
| Markdown | ATX headings, fenced code blocks with language labels, blank lines around lists and blocks. |

Let formatters decide wrapping and layout; do not add manual alignment that fights their output. Do not reformat generated files, lockfiles, or vendored code by hand.

Shared formatter/linter configuration includes the Svelte formatter integration and an `.editorconfig`. Keep editor settings and CI consistent with the committed configuration.

## 3. Naming

| Item | Convention | Example |
| --- | --- | --- |
| Rust modules, files, functions, fields, variables | `snake_case` | `provider_registry`, `start_proxy` |
| Rust types and enum variants | `PascalCase` | `ProviderConfig`, `ProxyState::Running` |
| Rust constants | `SCREAMING_SNAKE_CASE` | `MAX_ACTIVE_REQUESTS` |
| TypeScript variables, functions, object properties | `camelCase` | `providerConfig`, `startProxy` |
| TypeScript types and interfaces | `PascalCase` | `ProviderStatus` |
| TypeScript module constants | `SCREAMING_SNAKE_CASE` for fixed shared values | `DEFAULT_LOCAL_PORT` |
| Svelte component files | `PascalCase.svelte` | `ProviderSettings.svelte` |
| Other frontend files | `kebab-case` | `proxy-client.ts`, `provider-status.test.ts` |
| CSS classes | `kebab-case` | `connection-status` |

Use domain names such as `upstreamBaseUrl`, `apiKey`, and `providerId`. Avoid vague names such as `data`, `manager`, or `helper` when a more precise name fits. Use positive boolean names such as `isRunning` and `hasKey`.

Include units in values that would otherwise be ambiguous: `timeoutMs`, `maxBodyBytes`, or Rust `Duration` values. Keep limits and defaults in named, centralized configuration rather than scattered literals.

## 4. Rust conventions

- Prefer enums for finite states and provider identities. Avoid string comparisons and combinations of booleans that allow contradictory states.
- Use structs for related values and constructors or validation functions for invariants such as a valid endpoint or nonzero port.
- Keep items private unless another module needs them; prefer `pub(crate)` over a public library API when appropriate.
- Borrow values when possible. Clone deliberately, especially for strings, secrets, buffers, and configuration snapshots in the request path.
- Use immutable bindings by default. Introduce shared mutable state only where ownership requires it.
- Use exhaustive matches for application states. Avoid catch-all branches that conceal new variants.
- Prefer typed error enums for library and domain failures. Attach useful, sanitized context at application boundaries.
- Propagate recoverable errors with `Result` and `?`. Do not use `unwrap()`, `expect()`, `panic!()`, `todo!()`, or `unimplemented!()` for user input, network failures, credential access, or normal lifecycle transitions.
- In tests, `expect()` is acceptable for fixture setup and assertions with useful failure messages. Any production invariant assertion must have a documented reason it cannot fail from external input.
- Avoid `unsafe`. When native integration requires it, isolate it in a small platform adapter and document the safety invariant at each unsafe operation.

Keep HTTP adapters, GUI command handlers, and domain logic separate. A handler validates and translates inputs, calls the relevant service, and maps its result; it should not become the implementation of the entire feature.

## 5. Async, streaming, and resource ownership

- Reuse the application runtime and HTTP clients. Do not create a runtime or connection pool per request.
- Keep blocking filesystem, OS credential, and platform operations off async networking workers. Use a bounded blocking boundary where necessary.
- Do not hold a lock while awaiting network or credential operations. Take the required snapshot, release the guard, and then perform the work.
- Use bounded channels, queues, connection pools, and concurrency. Every long-lived task needs an owner and an explicit shutdown path.
- Avoid untracked fire-and-forget tasks. Propagate cancellation and observe task failures.
- Release concurrency permits when the forwarded response finishes or is dropped, rather than when response headers arrive.
- Stream bodies with backpressure. Do not collect full prompts, uploads, or responses for forwarding, logging, or UI updates.
- Keep finite connection/header deadlines separate from streaming inactivity limits. Do not silently retry generation requests.
- Prefer events over frequent polling. Use bounded backoff for recovery work and stop it when the app quits.

Opening and closing the GUI must clean up listeners, subscriptions, timers, and webview resources. Closing or quitting settings releases the GUI and preserves the worker. Stopping the proxy or exiting the worker releases networking resources. Keep GUI and worker lifecycles separate.

## 6. TypeScript and Svelte conventions

- Enable TypeScript strict checking. Avoid explicit `any`; use `unknown` at untrusted boundaries and narrow it through validation.
- Give shared IPC functions and exported module functions explicit input and return types. Allow inference for obvious local values.
- Use discriminated unions for asynchronous and lifecycle states. Represent loading, success, and failure explicitly.
- Keep components focused on presentation and interaction. Put IPC calls and reusable application behavior in typed modules.
- Use the idiomatic syntax of the Svelte version selected during scaffolding. Do not mix legacy and newer state patterns within a feature without a migration reason.
- Derive display values from authoritative state rather than keeping duplicate writable copies.
- Use event subscriptions for backend status and dispose of them on component teardown. Handle the case where asynchronous subscription setup finishes after teardown.
- Catch expected failures at the UI boundary, restore busy controls reliably, and show an actionable message. Do not swallow rejected promises.
- Use semantic HTML, explicit form labels, keyboard-accessible controls, and text for status changes. Do not rely on color alone.
- Keep styling in component styles or shared tokens. Avoid a large UI dependency for a small form or status indicator.

Secrets belong in transient input state only while needed. Never put them in frontend persistence, global debug state, analytics, or ordinary status objects. Clear key fields after successful storage and on teardown; do not automatically read saved secrets into forms.

## 7. IPC contracts and component boundaries

- Keep the proxy core independent of Tauri, Svelte, native tray APIs, and webview state.
- Route desktop-specific behavior through the platform adapters described in the architecture.
- Expose small, typed commands. Validate every command argument in Rust even if the GUI already checked it.
- Use Rust `snake_case` internally and `camelCase` for serialized GUI DTO fields. Configure serialization explicitly, for example with `#[serde(rename_all = "camelCase")]`.
- Keep command identifiers in `snake_case` and wrap them in named TypeScript functions. Do not scatter raw invocation strings across components.
- Use dedicated DTOs for settings, status, and errors. Do not serialize internal structs containing credentials, client handles, or OS objects.
- Return stable error codes and a safe message; let the UI choose a useful recovery action. Do not expose raw debug output or stack traces to users.
- Maintain one reviewed contract for each IPC shape. Prefer generated frontend types once a suitable tool is selected; otherwise keep paired Rust/TypeScript definitions synchronized.

For example, a provider status can contain `providerId`, `hasKey`, and `lastTestStatus`. Reading a key requires a separate, deliberate operation; it must not be a side effect of reading status.

## 8. Security and diagnostics

- Read and write credentials only through `CredentialStore`. Use non-secret credential identifiers in normal configuration.
- Do not derive or implement ordinary debug/serialization output that exposes secret fields. Mark sensitive tracing arguments to be skipped and log safe fields explicitly.
- Keep keys, tokens, authorization headers, prompts, responses, and full URLs out of logs and error context. A query string can contain sensitive data.
- Validate endpoints and paths with URL-aware code. Avoid URL construction by unchecked string concatenation.
- Preserve the configured destination boundary and certificate validation. Do not add permissive TLS or authentication fallbacks to make a failing test pass.
- Name local authentication and upstream header construction clearly; both use the selected provider's saved API key. Keep optional provider metadata separate from authentication.
- Keep logs structured, bounded, and useful: request ID, provider, safe error code, HTTP status, and duration are appropriate fields.
- Do not add IPC permissions, shell access, filesystem scope, or dependencies beyond what the feature needs.

Log an operational failure at the boundary responsible for handling it. Lower layers should return useful errors without logging the same failure repeatedly.

## 9. Cross-platform code and dependencies

- Use platform path APIs and application-data directories; never hard-code a home directory or path separator.
- Isolate macOS/Windows differences in platform modules with targeted conditional compilation. Keep conditional branches out of shared forwarding logic where possible.
- Use descriptive platform-independent errors in shared code and map OS-specific details inside adapters.
- Keep filenames portable across case-insensitive filesystems and Windows filename rules.
- Use npm for frontend dependencies and commit its lockfile. Commit `Cargo.lock` for the application workspace.
- Add dependencies only when their functionality justifies their binary size, runtime cost, maintenance, and platform support. Enable only required features.
- Keep secrets and machine-specific paths out of repository configuration and fixtures.
- Use cross-platform tooling for npm tasks; do not assume a POSIX shell on Windows.

## 10. Comments and documentation

Comments should explain an invariant, tradeoff, platform behavior, or surprising implementation detail. Avoid comments that restate the code.

Document public Rust library interfaces and shared IPC contracts when their behavior is not obvious, especially cancellation, credential handling, units, and error guarantees. Update the relevant project document when a change alters an architectural or product decision.

Use `TODO` only with a concrete missing behavior and an issue reference or clear completion condition. Do not leave unresolved placeholders in release paths. Examples and fixtures must use unmistakably fake credentials.

## 11. Tests and verification

- Test observable behavior and important failure paths. Avoid tests that merely repeat a helper's implementation or assert incidental component markup.
- Put Rust unit tests near the logic and cross-module integration tests under the owning crate's `tests/` directory.
- Use descriptive test names such as `rejects_conflicting_local_credentials` or `closing_settings_preserves_proxy`.
- Use deterministic local upstream fixtures for forwarding tests. Cover path mapping, credential replacement, streaming before completion, cancellation, limits, safe headers, and errors.
- Avoid fixed sleeps when an event, readiness signal, or bounded wait can establish the condition.
- Keep normal tests independent of internet access, real credentials, paid requests, and persistent user settings. Isolate OS integration tests and clean up their resources.
- Add frontend tests for meaningful interaction and state behavior. A reversible styling-only change does not require a new test by itself.
- Measure memory changes in release builds using the architecture's workload and include helper processes. Report measured results separately from expectations.

Run the following shared checks:

| Area | Expected checks |
| --- | --- |
| Rust formatting | `cargo fmt --all -- --check` |
| Rust lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| Rust behavior | `cargo test --workspace --locked` |
| Frontend formatting | `npm run format:check` |
| Frontend lint | `npm run lint` |
| TypeScript/Svelte checks | `npm run check` |
| Frontend behavior | `npm test` for the configured suite. |

These commands are configured in the repository and CI. Run platform checks on supported hosts, and do not blindly enable mutually exclusive platform features in a single build. Completed local validation is recorded in `VALIDATION.md`.

Fix warnings instead of disabling a lint across the repository. A narrowly scoped suppression must include the reason it is necessary. Run checks appropriate to the change; distinguish completed checks from checks that require another platform or are not yet configured.

## 12. Review expectations

A change description should state the user-visible behavior, the implementation decision that matters, relevant validation, and any unresolved limitation. For resource-sensitive changes, include memory or CPU measurements when applicable. For credential, routing, or lifecycle changes, explain the invariant being preserved and its verification.

Before merging code, check that naming and formatting follow this guide, error paths are handled, owned resources are released, secrets remain protected, and the implementation stays within the documented scope. Keep review focused on correctness, clarity, and measurable constraints rather than personal formatting preferences.
