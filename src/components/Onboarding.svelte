<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import {
    httpsReady,
    setupOnboardingHttps,
    setupStep,
    skipOnboarding,
    type SetupStep,
  } from '../lib/onboarding';
  import { errorMessage, providerName } from '../lib/presentation';
  import { proxyClient } from '../lib/proxy-client';
  import type { AppStatus, ProviderId } from '../lib/types';

  interface Props {
    status: AppStatus;
    disabled: boolean;
    onStatus: (status: AppStatus) => void;
    onNotice: (message: string, isError?: boolean) => void;
    onSettings: () => void;
    onSkipped: () => void;
  }

  let { status, disabled, onStatus, onNotice, onSettings, onSkipped }: Props = $props();
  let selected = $state<ProviderId>(
    untrack(() => status.providers.find((item) => item.hasKey)?.id ?? 'claude'),
  );
  let apiKey = $state('');
  let reveal = $state(false);
  let endpoint = $state(
    untrack(() => status.providers.find((item) => item.id === selected)!.upstreamBaseUrl),
  );
  let busy = $state(false);
  let previousStep = $state<SetupStep | null>(null);
  const step = $derived(previousStep ?? setupStep(status));
  const locked = $derived(disabled || busy);
  const provider = $derived(status.providers.find((item) => item.id === selected)!);
  const approvalRequired = $derived(status.backgroundStartup === 'approval_required');
  const labels = ['Provider key', 'Trusted HTTPS', 'Background proxy'];

  onDestroy(() => {
    apiKey = '';
  });

  function selectProvider(id: ProviderId): void {
    selected = id;
    apiKey = '';
    reveal = false;
    endpoint = status.providers.find((item) => item.id === id)!.upstreamBaseUrl;
  }

  async function perform(action: () => Promise<void>): Promise<void> {
    busy = true;
    onNotice('');
    try {
      await action();
    } catch (error) {
      // Certificate creation may have succeeded before trust was canceled. Resume from reality.
      const latest = await proxyClient.getStatus().catch(() => null);
      if (latest) onStatus(latest);
      onNotice(errorMessage(error), true);
    } finally {
      busy = false;
    }
  }

  async function saveKey(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    await perform(async () => {
      const next = await proxyClient.saveProvider(selected, endpoint.trim(), apiKey.trim() || null);
      apiKey = '';
      reveal = false;
      endpoint = next.providers.find((item) => item.id === selected)!.upstreamBaseUrl;
      onStatus(next);
      previousStep = null;
    });
  }

  async function setupHttps(): Promise<void> {
    await perform(async () => {
      const next = await setupOnboardingHttps(status, proxyClient);
      onStatus(next);
      if (httpsReady(next)) previousStep = null;
      else
        onNotice('Certificate trust is not verified yet. Allow the OS prompt, then retry.', true);
    });
  }

  async function finish(): Promise<void> {
    await perform(async () => {
      const next = await proxyClient.finishOnboarding();
      onStatus(next);
      if (next.onboardingComplete) {
        onNotice(
          next.backgroundStartup === 'development'
            ? 'Setup complete. The proxy is running. Development builds do not register background startup.'
            : 'Setup complete. The proxy is running and will start in the background after you sign in.',
        );
      } else if (next.backgroundStartup === 'approval_required') {
        onNotice(
          'The proxy is running. Allow background startup in macOS settings to finish setup.',
        );
      } else {
        onNotice(
          'The proxy is running, but background startup needs attention before setup can finish.',
          true,
        );
      }
    });
  }

  async function openStartupSettings(): Promise<void> {
    await perform(async () => {
      await proxyClient.openStartupSettings();
      onNotice('Enable ICA proxy under Allow in the Background. Return here and check approval.');
    });
  }

  async function skip(): Promise<void> {
    await perform(async () => {
      const next = await skipOnboarding(
        status,
        { id: selected, upstreamBaseUrl: endpoint, apiKey },
        proxyClient,
        (saved) => {
          apiKey = '';
          reveal = false;
          onStatus(saved);
        },
      );
      apiKey = '';
      reveal = false;
      onStatus(next);
      onSkipped();
      onNotice(
        'Setup skipped. Your entered details are saved. You can resume setup or start the proxy from settings.',
      );
    });
  }
</script>

<section class="onboarding" aria-labelledby="setup-heading" aria-busy={busy}>
  <div class="setup-intro">
    <p class="eyebrow">FIRST-TIME SETUP</p>
    <h1 id="setup-heading">Set up ICA proxy</h1>
    <p>Connect a provider, secure localhost, then let the proxy run in the background.</p>
  </div>

  <ol class="setup-progress" aria-label="Setup progress">
    {#each labels as label, index (label)}
      <li
        class:current={step === index + 1}
        class:done={step > index + 1}
        aria-current={step === index + 1 ? 'step' : undefined}
      >
        <span class="step-number" aria-hidden="true">{step > index + 1 ? '✓' : index + 1}</span>
        <span>{label}</span>
      </li>
    {/each}
  </ol>

  <div class="setup-body">
    {#if step === 1}
      <h2>Connect your provider</h2>
      <p>One API key is enough to get started. You can add the other provider in settings later.</p>
      <form onsubmit={saveKey}>
        <fieldset disabled={locked}>
          <legend>Choose a provider</legend>
          <div class="provider-choice">
            {#each ['claude', 'openai'] as id (id)}
              <label class:chosen={selected === id}>
                <input
                  type="radio"
                  name="setup-provider"
                  value={id}
                  checked={selected === id}
                  onchange={() => selectProvider(id as ProviderId)}
                />
                {providerName(id as ProviderId)}
              </label>
            {/each}
          </div>
        </fieldset>
        <label for="setup-api-key">{providerName(selected)} API key</label>
        <div class="key-input">
          <input
            id="setup-api-key"
            type={reveal ? 'text' : 'password'}
            bind:value={apiKey}
            placeholder={provider.hasKey
              ? 'Key already saved · enter a new key to replace it'
              : 'Paste your API key'}
            required={!provider.hasKey}
            autocomplete="off"
            autocapitalize="off"
            spellcheck="false"
            disabled={locked}
            aria-describedby="setup-key-hint"
          />
          <button
            type="button"
            class="text-button"
            onclick={() => (reveal = !reveal)}
            disabled={locked}
            aria-pressed={reveal}>{reveal ? 'Hide' : 'Show'}</button
          >
        </div>
        <p id="setup-key-hint" class="field-hint">
          Stored in your OS credential store. Use this same key when connecting your API client.
        </p>
        <details class="endpoint-details">
          <summary>Provider endpoint</summary>
          <label for="setup-endpoint">Upstream base URL</label>
          <input
            id="setup-endpoint"
            type="url"
            bind:value={endpoint}
            required
            disabled={locked}
            spellcheck="false"
            autocapitalize="off"
          />
        </details>
        {#if status.platform === 'macos'}
          <p class="os-note">
            If Keychain asks for access, choose <strong>Always Allow</strong> for each ICA proxy worker
            key prompt. A locked Keychain still needs your password.
          </p>
        {/if}
        <button
          class="button primary"
          type="submit"
          disabled={locked || (!apiKey.trim() && !provider.hasKey)}
          >{busy ? 'Saving key…' : 'Save key and continue'}</button
        >
      </form>
    {:else if step === 2}
      <h2>Secure your local connection</h2>
      <p>
        Create a certificate for localhost and trust it on this computer so Claude for Office can
        connect over HTTPS.
      </p>
      <div class="setup-detail">
        <strong
          >{status.certificate.configured ? 'Certificate saved' : 'Local HTTPS certificate'}</strong
        >
        <p>
          {status.certificate.configured && !status.certificate.trusted
            ? 'Your certificate is saved. Retry to finish trusting it.'
            : 'Certificate setup and trust happen together. Approve the operating system prompt when it appears.'}
        </p>
        <code>https://localhost:{status.httpsPort}</code>
      </div>
      <p class="os-note">
        The OS may ask for your password or approval. Only approve the ICA proxy localhost
        certificate on a computer you trust.
      </p>
      <button class="button primary" onclick={setupHttps} disabled={locked}
        >{busy ? 'Setting up trusted HTTPS…' : 'Set up HTTPS and trust certificate'}</button
      >
    {:else}
      <h2>Keep the proxy ready</h2>
      <p>
        Start your secure proxy now and enable automatic background startup after you sign in. The
        settings window stays closed at sign-in.
      </p>
      <div class="setup-detail">
        <strong>{status.phase === 'running' ? 'Proxy running' : 'Ready to start'}</strong>
        <p>Your provider key is saved and the HTTPS certificate is trusted.</p>
        {#each status.providers.filter((item) => item.hasKey) as item (item.id)}
          <code>{item.httpsBaseUrl}</code>
        {/each}
      </div>
      {#if approvalRequired}
        <p class="os-note">
          macOS needs your approval. Open Login Items settings and enable ICA proxy under <strong
            >Allow in the Background</strong
          >, then return here. Checking approval may briefly restart the running proxy.
        </p>
        <button class="button secondary" onclick={openStartupSettings} disabled={locked}
          >Open Login Items settings</button
        >
      {:else if status.backgroundStartup === 'installation_required'}
        <p class="os-note">
          Move the complete ICA proxy app to Applications and open that copy to enable background
          startup.
        </p>
      {:else if status.backgroundStartup === 'disabled'}
        <p class="os-note">
          Background startup is disabled. Allow ICA proxy in your OS background-app settings, then
          retry. On Windows, check its llm_proxy task in Task Scheduler.
        </p>
      {:else if status.backgroundStartup === 'development'}
        <p class="os-note">
          Development build: the proxy can run, but automatic background startup is available only
          in the installed app.
        </p>
      {/if}
      <button class="button primary" onclick={finish} disabled={locked}
        >{busy
          ? 'Starting and checking…'
          : approvalRequired
            ? 'Check approval and finish setup'
            : 'Start proxy and enable background startup'}</button
      >
    {/if}
  </div>
  <div class="setup-footer">
    <div>
      {#if step > 1}<button
          class="text-button"
          onclick={() => (previousStep = (step - 1) as SetupStep)}
          disabled={locked}>Back</button
        >{:else}<span>Your keys stay on this device.</span>{/if}
    </div>
    <div class="setup-footer-actions">
      <button class="text-button" onclick={onSettings} disabled={locked}>Open settings</button>
      <button type="button" class="text-button" onclick={skip} disabled={locked}>Skip setup</button>
    </div>
  </div>
  <p class="skip-hint">
    Skipping saves your entered details and opens settings. Finish the remaining steps later.
  </p>
</section>

<style>
  .onboarding {
    max-width: 720px;
    margin: 8px auto 24px;
  }
  .setup-intro {
    padding: 0 0 20px;
  }
  .eyebrow {
    color: var(--accent);
    font-size: 11px;
    font-weight: 750;
    letter-spacing: 0.13em;
    margin: 0 0 12px;
  }
  h1 {
    font-size: 28px;
    line-height: 1.18;
    max-width: 580px;
    letter-spacing: -0.025em;
    margin: 0 0 14px;
  }
  p {
    line-height: 1.6;
    color: var(--muted);
  }
  .setup-intro > p:last-child {
    margin: 0;
  }
  .setup-progress {
    display: flex;
    padding: 0 0 24px;
    margin: 0;
    list-style: none;
    gap: 16px;
  }
  .setup-progress li {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    font-size: 12px;
    color: var(--muted);
  }
  .step-number {
    display: grid;
    place-items: center;
    width: 27px;
    height: 27px;
    border: 1px solid var(--line);
    border-radius: 50%;
    flex: 0 0 auto;
    font-weight: 700;
  }
  .current {
    color: inherit !important;
    font-weight: 700;
  }
  .current .step-number {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }
  .done .step-number {
    background: #e6f1ec;
    color: #226746;
    border-color: #c1dccd;
  }
  .setup-body {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 14px;
    padding: 24px;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 21px;
  }
  .setup-body > p {
    margin: 0 0 24px;
  }
  form {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
  }
  form > label,
  form > .key-input,
  form > details,
  fieldset {
    width: 100%;
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0 0 12px;
  }
  legend {
    font-size: 12px;
    font-weight: 650;
    margin-bottom: 10px;
  }
  .provider-choice {
    display: flex;
    gap: 12px;
  }
  .provider-choice label {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: 1;
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 14px;
    cursor: pointer;
  }
  .provider-choice label.chosen {
    border-color: var(--accent);
    background: #f1f6fd;
  }
  input[type='radio'] {
    width: 16px;
    height: 16px;
    margin: 0;
    accent-color: var(--accent);
  }
  .key-input {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .key-input input {
    flex: 1;
    min-width: 0;
  }
  .field-hint {
    margin: 0 0 6px;
  }
  .endpoint-details {
    margin: 2px 0 10px;
    font-size: 13px;
  }
  summary {
    cursor: pointer;
    color: var(--muted);
    padding: 5px 0;
  }
  .endpoint-details label {
    display: block;
    margin: 12px 0 8px;
  }
  .endpoint-details input {
    width: 100%;
  }
  .os-note {
    font-size: 12px;
    line-height: 1.65;
    margin: 12px 0 22px !important;
  }
  .setup-detail {
    padding: 20px;
    border-left: 3px solid var(--accent);
    background: #f4f7fc;
    margin: 24px 0;
  }
  .setup-detail p {
    margin: 5px 0 12px;
    font-size: 13px;
  }
  code {
    display: block;
    overflow-wrap: anywhere;
    margin: 6px 0;
    font-size: 13px;
  }
  .setup-body > .button {
    margin: 0 8px 8px 0;
  }
  .setup-footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 18px 4px;
    color: var(--muted);
    font-size: 12px;
  }
  .setup-footer-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
  }
  .skip-hint {
    font-size: 12px;
    margin: 0 4px;
  }
  @media (max-width: 560px) {
    .setup-body {
      padding: 22px;
    }
    .setup-progress {
      gap: 6px;
    }
    .setup-progress li {
      flex-direction: column;
      align-items: flex-start;
      font-size: 11px;
    }
    .button.primary {
      width: 100%;
      white-space: normal;
    }
  }
  @media (prefers-color-scheme: dark) {
    .provider-choice label.chosen {
      background: #2c4059;
    }
    .setup-detail {
      background: #283b53;
    }
    .current .step-number {
      color: #1d2734;
    }
    .done .step-number {
      background: #253e35;
      color: #84ceaa;
      border-color: #3c6654;
    }
  }
</style>
