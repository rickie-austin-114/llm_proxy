<script lang="ts">
  import { onMount } from 'svelte';
  import Onboarding from './components/Onboarding.svelte';
  import ProviderSettings from './components/ProviderSettings.svelte';
  import ConfirmationDialog from './components/ConfirmationDialog.svelte';
  import HttpsSettings from './components/HttpsSettings.svelte';
  import { isDesktop, proxyClient } from './lib/proxy-client';
  import { errorMessage, parsePort, previewStatus, providerName } from './lib/presentation';
  import type { AppStatus } from './lib/types';

  const desktop = isDesktop();
  let status = $state<AppStatus>(previewStatus());
  let loading = $state(desktop);
  let busy = $state(false);
  let settingsDuringSetup = $state(false);
  let resumeSkippedSetup = $state(false);
  let notice = $state('');
  let noticeIsError = $state(false);
  let portInput = $state('8787');
  let browserOriginsInput = $state(previewStatus().browserOrigins.join('\n'));
  let confirmationMessage = $state('');
  let confirmationHeading = $state('Confirm change');
  let confirmationLabel = $state('Continue');
  let confirmationCancelLabel = $state('Cancel');
  let startupApprovalPromptShown = false;
  let resolveConfirmation: ((confirmed: boolean) => void) | undefined;
  const showOnboarding = $derived(
    !loading &&
      !status.onboardingComplete &&
      (!status.onboardingSkipped || resumeSkippedSetup) &&
      !status.settingsNeedReset &&
      !settingsDuringSetup,
  );
  const running = $derived(status.phase === 'running');
  const transitioning = $derived(status.phase === 'starting' || status.phase === 'stopping');
  const disabled = $derived(!desktop || loading || busy || transitioning);
  const hasProvider = $derived(status.providers.some((provider) => provider.hasKey));
  const phaseLabel = $derived(
    {
      stopped: 'Proxy stopped',
      starting: 'Starting proxy',
      running: 'Proxy running',
      stopping: 'Stopping proxy',
      error: 'Proxy needs attention',
    }[status.phase],
  );

  function updateStatus(next: AppStatus): void {
    status = next;
    portInput = String(next.localPort);
    browserOriginsInput = next.browserOrigins.join('\n');
  }

  function notify(message: string, isError = false): void {
    notice = message;
    noticeIsError = isError;
  }

  function confirmAction(
    message: string,
    options: { heading?: string; confirmLabel?: string; cancelLabel?: string } = {},
  ): Promise<boolean> {
    if (resolveConfirmation) return Promise.resolve(false);
    confirmationHeading = options.heading ?? 'Confirm change';
    confirmationLabel = options.confirmLabel ?? 'Continue';
    confirmationCancelLabel = options.cancelLabel ?? 'Cancel';
    return new Promise((resolve) => {
      resolveConfirmation = resolve;
      confirmationMessage = message;
    });
  }

  function decideConfirmation(confirmed: boolean): void {
    const resolve = resolveConfirmation;
    resolveConfirmation = undefined;
    confirmationMessage = '';
    resolve?.(confirmed);
  }

  onMount(() => {
    if (!desktop) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const stop = await proxyClient.onStatus((next) => {
          if (!disposed) updateStatus(next);
        });
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
        const initial = await proxyClient.getStatus();
        if (!disposed) updateStatus(initial);
      } catch (error) {
        if (!disposed) notify(errorMessage(error), true);
      } finally {
        if (!disposed) loading = false;
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
      decideConfirmation(false);
    };
  });

  $effect(() => {
    if (
      desktop &&
      !loading &&
      !showOnboarding &&
      !status.onboardingSkipped &&
      !busy &&
      !confirmationMessage &&
      !startupApprovalPromptShown &&
      status.platform === 'macos' &&
      status.backgroundStartup === 'approval_required'
    ) {
      // Prompt only in an open settings session, once. The headless worker has no UI.
      startupApprovalPromptShown = true;
      void requestStartupApproval();
    }
  });

  async function perform(action: () => Promise<void>): Promise<void> {
    busy = true;
    notice = '';
    try {
      await action();
    } catch (error) {
      notify(errorMessage(error), true);
    } finally {
      busy = false;
    }
  }

  async function toggleProxy(): Promise<void> {
    if (!status.onboardingComplete && !status.onboardingSkipped && !running) {
      settingsDuringSetup = false;
      return;
    }
    await perform(async () => {
      updateStatus(await (running ? proxyClient.stop() : proxyClient.start()));
    });
  }

  async function openStartupSettings(): Promise<void> {
    await perform(async () => {
      await proxyClient.openStartupSettings();
      notify(
        'Allow ICA proxy under Allow in the Background in Login Items settings. Return here and select Check background startup.',
      );
    });
  }

  async function requestStartupApproval(): Promise<void> {
    if (
      await confirmAction(
        'Allow ICA proxy to start its proxy automatically after you sign in? macOS requires approval in Login Items settings. Enable ICA proxy under Allow in the Background, then return here and select Check background startup. The settings app stays closed at sign-in.',
        {
          heading: 'Allow background startup',
          confirmLabel: 'Open Login Items settings',
          cancelLabel: 'Later',
        },
      )
    ) {
      await openStartupSettings();
    }
  }

  async function checkBackgroundStartup(): Promise<void> {
    if (
      running &&
      !(await confirmAction(
        'Checking background startup may restart the proxy to hand it over to the operating system. This can interrupt active requests. Continue?',
      ))
    )
      return;
    await perform(async () => {
      const next = await proxyClient.checkBackgroundStartup();
      updateStatus(next);
      if (next.backgroundStartup === 'enabled') {
        notify('Background startup is enabled. Only the proxy worker starts after sign-in.');
      } else if (next.backgroundStartup === 'approval_required') {
        notify('macOS approval is still required. Open Login Items settings to allow ICA proxy.');
      } else if (next.backgroundStartup === 'disabled') {
        notify('Background startup is disabled. Your OS or reset preference has been preserved.');
      } else {
        notify('Background startup still needs attention. Check the message above.', true);
      }
    });
  }

  async function savePort(): Promise<void> {
    const port = parsePort(portInput);
    if (port === null) {
      notify('Choose a whole-number port between 1024 and 65535.', true);
      return;
    }
    await perform(async () => {
      updateStatus(await proxyClient.setPort(port));
      notify('Local port saved. Update the base URL in your API client.');
    });
  }

  async function saveBrowserOrigins(): Promise<void> {
    const origins = browserOriginsInput
      .split(/\r?\n/)
      .map((value) => value.trim())
      .filter(Boolean);
    await perform(async () => {
      updateStatus(await proxyClient.setBrowserOrigins(origins));
      notify('Trusted browser origins saved. Start the proxy to apply them.');
    });
  }

  async function copyUrl(value: string): Promise<void> {
    await perform(async () => {
      await proxyClient.copyText(value);
      notify('Local base URL copied.');
    });
  }

  async function checkSavedKeyAccess(): Promise<void> {
    await perform(async () => {
      await proxyClient.checkSavedKeyAccess();
      notify(
        'Saved keys are accessible now. macOS does not tell the app whether you selected Always Allow. Select Start proxy if it is stopped.',
      );
    });
  }

  async function reset(): Promise<void> {
    if (
      !(await confirmAction(
        'Stop the proxy, remove local HTTPS trust and its private key, delete both provider keys, reset settings, and disable automatic startup? This also prepares the app for uninstall.',
      ))
    )
      return;
    await perform(async () => {
      updateStatus(await proxyClient.reset());
      settingsDuringSetup = false;
      resumeSkippedSetup = false;
      notify(
        'App data removed and automatic startup disabled. You can set up again or uninstall the app.',
      );
    });
  }
</script>

<svelte:head><title>ICA proxy · Connection settings</title></svelte:head>

<div class="app-shell">
  <header class="app-header">
    <div class="brand">
      <svg class="brand-mark" viewBox="0 0 40 40" fill="none" aria-hidden="true">
        <rect x="1" y="1" width="38" height="38" rx="11" fill="currentColor" />
        <path
          d="M11 13h6v14h-6m18-14h-6v14h6M17 20h6"
          stroke="white"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      </svg>
      <div>
        <span class="brand-name">ICA proxy</span><span class="brand-description"
          >Claude & OpenAI, on localhost</span
        >
      </div>
    </div>
    <span class="device-badge"><span aria-hidden="true">⌂</span> On this device</span>
  </header>

  <main>
    {#if !desktop}
      <div class="notice preview" role="status">
        Browser preview. Open the desktop app to save keys and run the proxy.
      </div>
    {/if}
    <div class="notices" aria-live="polite" aria-atomic="true">
      {#if notice}<div class="notice" class:error={noticeIsError}>{notice}</div>{/if}
      {#if status.lastError}<div class="notice error">{status.lastError.message}</div>{/if}
    </div>
    {#if showOnboarding}
      <Onboarding
        {status}
        {disabled}
        onStatus={updateStatus}
        onNotice={notify}
        onSettings={() => (settingsDuringSetup = true)}
        onSkipped={() => {
          settingsDuringSetup = true;
          resumeSkippedSetup = false;
        }}
      />
    {:else}
      {#if !loading && !status.onboardingComplete && !status.settingsNeedReset}
        <div class="notice" role="status">
          {status.onboardingSkipped
            ? 'Setup skipped. Configure the proxy below or finish guided setup.'
            : 'Setup is not complete.'}
          <button
            class="text-button"
            onclick={() => {
              settingsDuringSetup = false;
              resumeSkippedSetup = true;
            }}
            {disabled}>Resume setup</button
          >
        </div>
      {/if}
      <section class="proxy-strip" class:running aria-labelledby="proxy-heading">
        <div class="proxy-state">
          <span class="status-light" aria-hidden="true"></span>
          <div>
            <h1 id="proxy-heading">{loading ? 'Loading settings' : phaseLabel}</h1>
            <p>
              {running
                ? `Listening on port ${status.localPort}. Ready for your API clients.`
                : 'Save a provider key, then start your local connection.'}
            </p>
          </div>
        </div>
        <button
          class="button primary"
          class:stop-button={running}
          onclick={toggleProxy}
          disabled={disabled ||
            (!hasProvider && !running && (status.onboardingComplete || status.onboardingSkipped)) ||
            status.settingsNeedReset}
          >{transitioning
            ? 'Please wait…'
            : running
              ? 'Stop proxy'
              : !status.onboardingComplete && !status.onboardingSkipped
                ? 'Resume setup'
                : 'Start proxy'}</button
        >
      </section>

      <section class="providers-section" aria-labelledby="providers-heading">
        <div class="section-intro">
          <h2 id="providers-heading">Provider connections</h2>
          <p>Keys stay in your OS credential store. Requests go to the endpoint you choose.</p>
        </div>
        {#if !loading && status.platform === 'macos'}
          <aside class="keychain-help" aria-labelledby="keychain-heading">
            <h3 id="keychain-heading">Remember Keychain access on this Mac</h3>
            <p>
              When macOS asks llm-proxy-worker to access its saved keys, enter your login Keychain
              password (usually your Mac login password) and choose <strong>Always Allow</strong>.
              The background worker may need approval once after this update. Approve each prompt;
              Claude, OpenAI, and the HTTPS private key are separate items.
            </p>
            <button
              class="button secondary small"
              onclick={checkSavedKeyAccess}
              disabled={disabled ||
                status.settingsNeedReset ||
                (!hasProvider && !status.httpsEnabled)}>Check saved-key access</button
            >
            <p class="field-hint">
              This reads saved keys without contacting your providers. A locked Keychain still needs
              unlocking. App updates may require approval again. macOS controls remembered approval;
              the app cannot select Always Allow for you.
            </p>
          </aside>
        {/if}
        <div class="providers-grid">
          {#each status.providers as provider (provider.id)}
            <ProviderSettings
              {provider}
              disabled={disabled || status.settingsNeedReset}
              onStatus={updateStatus}
              onNotice={notify}
              onConfirm={confirmAction}
            />
          {/each}
        </div>
      </section>

      <HttpsSettings
        {status}
        disabled={disabled || status.settingsNeedReset}
        {running}
        onStatus={updateStatus}
        onNotice={notify}
        onConfirm={confirmAction}
        {perform}
      />

      <section class="client-section" aria-labelledby="client-heading">
        <div class="section-intro">
          <h2 id="client-heading">Connect your client</h2>
          <p>Use the matching local base URL and the same API key saved for that provider above.</p>
        </div>
        <div class="connection-table">
          {#each status.providers as provider (provider.id)}
            {#if provider.httpsBaseUrl}
              <div class="connection-row">
                <span class="connection-name">{providerName(provider.id)} HTTPS</span>
                <input
                  class="url-value"
                  aria-label={`${providerName(provider.id)} HTTPS base URL`}
                  readonly
                  value={provider.httpsBaseUrl}
                />
                <button
                  class="text-button"
                  onclick={() => copyUrl(provider.httpsBaseUrl!)}
                  {disabled}>Copy URL</button
                >
              </div>
            {/if}
            <div class="connection-row">
              <span class="connection-name">{providerName(provider.id)}</span><input
                class="url-value"
                aria-label={`${providerName(provider.id)} local base URL`}
                readonly
                value={provider.localBaseUrl}
              /><button
                class="text-button"
                onclick={() => copyUrl(provider.localBaseUrl)}
                {disabled}>Copy URL</button
              >
            </div>
          {/each}
        </div>
        <p class="field-hint">
          Claude requests use your saved Claude API key; OpenAI requests use your saved OpenAI API
          key. Only connections from this computer are accepted.
        </p>
      </section>

      <section class="preferences" aria-labelledby="preferences-heading">
        <h2 id="preferences-heading">Background & startup</h2>
        <div class="preferences-row">
          <div class="startup-description">
            <strong>Automatic background proxy</strong>
            <p>
              {status.backgroundStartup === 'enabled'
                ? 'The proxy starts after you sign in. This settings app stays closed.'
                : status.backgroundStartup === 'approval_required'
                  ? 'Allow ICA proxy under Login Items & Extensions in macOS System Settings.'
                  : status.backgroundStartup === 'disabled'
                    ? status.platform === 'windows'
                      ? 'Background startup is disabled. Check the task whose name starts with llm_proxy- in Windows Task Scheduler, or resume setup after resetting.'
                      : 'Background startup is disabled. Check macOS Login Items & Extensions, or resume setup after resetting.'
                    : status.backgroundStartup === 'development'
                      ? 'Development mode: automatic startup is not registered.'
                      : status.backgroundStartup === 'setup_required'
                        ? status.onboardingSkipped
                          ? 'Start the proxy or resume guided setup to enable background startup.'
                          : 'Finish first-time setup to enable background startup.'
                        : status.backgroundStartup === 'installation_required'
                          ? 'Install the complete ICA proxy app in Applications and open that copy to configure background startup.'
                          : 'Background startup needs attention. Check the error above, then select Check background startup to retry.'}
            </p>
            {#if status.backgroundStartup !== 'development'}
              {#if status.platform === 'macos'}
                <button class="button secondary small" onclick={openStartupSettings} {disabled}
                  >Open Login Items settings</button
                >
              {/if}
              <button
                class="button secondary small"
                onclick={checkBackgroundStartup}
                disabled={disabled || status.settingsNeedReset}>Check background startup</button
              >
            {/if}
          </div>
          <div class="port-setting">
            <label for="local-port">Local port</label>
            <div class="port-controls">
              <input
                id="local-port"
                inputmode="numeric"
                bind:value={portInput}
                disabled={disabled || running || status.settingsNeedReset}
              /><button
                class="button secondary small"
                onclick={savePort}
                disabled={disabled ||
                  running ||
                  portInput === String(status.localPort) ||
                  status.settingsNeedReset}>Apply</button
              >
            </div>
          </div>
        </div>
        <p class="field-hint">
          Closing or quitting settings leaves the proxy running. Open ICA proxy from Applications or
          the Start menu to return here. Use Stop proxy to stop requests until the worker starts
          again after sign-in.
        </p>
      </section>

      <details class="diagnostics browser-access">
        <summary>Trusted browser origins</summary>
        <p>
          Allow Claude for Office and other trusted browser clients. Each client still needs your
          saved provider API key.
        </p>
        <label for="browser-origins">HTTPS origins, one per line</label>
        <textarea
          id="browser-origins"
          rows="3"
          spellcheck="false"
          autocapitalize="off"
          aria-describedby="browser-origins-hint"
          bind:value={browserOriginsInput}
          disabled={disabled || running || status.settingsNeedReset}
        ></textarea>
        <p id="browser-origins-hint" class="field-hint">
          Default: https://pivot.claude.ai. Stop the proxy to edit. Use exact origins without paths,
          trailing slashes, or wildcards. Leave empty to disable browser access. Add future Office
          origins only when documented by Anthropic.
        </p>
        <button
          class="button secondary small"
          onclick={saveBrowserOrigins}
          disabled={disabled ||
            running ||
            status.settingsNeedReset ||
            browserOriginsInput === status.browserOrigins.join('\n')}>Save browser origins</button
        >
      </details>

      <details class="diagnostics">
        <summary>Diagnostics <span>{status.diagnostics.length} recent events</span></summary
        >{#if status.diagnostics.length === 0}<p>
            No events yet. Diagnostics contain status and errors, never your prompts or keys.
          </p>{:else}<ol>
            {#each status.diagnostics as event (event.id)}<li>
                <time>{new Date(event.timestamp * 1000).toLocaleTimeString()}</time><span
                  >{event.message}</span
                >
              </li>{/each}
          </ol>{/if}
      </details>
      <footer>
        <span>Private by default. No account or hosted relay.</span><button
          class="text-button danger"
          onclick={reset}
          {disabled}>Reset data / prepare to uninstall</button
        >
      </footer>
    {/if}
  </main>
</div>

<ConfirmationDialog
  message={confirmationMessage}
  heading={confirmationHeading}
  confirmLabel={confirmationLabel}
  cancelLabel={confirmationCancelLabel}
  onDecision={decideConfirmation}
/>
