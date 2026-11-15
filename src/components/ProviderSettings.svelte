<script lang="ts">
  import { proxyClient } from '../lib/proxy-client';
  import { errorMessage, providerName } from '../lib/presentation';
  import type { AppStatus, ProviderStatus } from '../lib/types';

  let {
    provider,
    disabled,
    onStatus,
    onNotice,
    onConfirm,
  }: {
    provider: ProviderStatus;
    disabled: boolean;
    onStatus: (status: AppStatus) => void;
    onNotice: (message: string, isError?: boolean) => void;
    onConfirm: (message: string) => Promise<boolean>;
  } = $props();

  let endpoint = $state('');
  let apiKey = $state('');
  let isDirty = $state(false);
  let isRevealed = $state(false);
  let busy = $state(false);
  const name = $derived(providerName(provider.id));
  const locked = $derived(disabled || busy);

  $effect(() => {
    if (!isDirty) endpoint = provider.upstreamBaseUrl;
  });

  async function perform(action: () => Promise<void>): Promise<void> {
    busy = true;
    try {
      await action();
    } catch (error) {
      onNotice(errorMessage(error), true);
    } finally {
      busy = false;
    }
  }

  async function save(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    await perform(async () => {
      const status = await proxyClient.saveProvider(provider.id, endpoint.trim(), apiKey || null);
      apiKey = '';
      isRevealed = false;
      isDirty = false;
      onStatus(status);
      onNotice(`${name} settings saved.`);
    });
  }

  async function test(): Promise<void> {
    await perform(async () => {
      const result = await proxyClient.testConnection(provider.id, endpoint.trim(), apiKey || null);
      onNotice(result);
    });
  }

  async function reveal(): Promise<void> {
    if (isRevealed) {
      isRevealed = false;
      return;
    }
    if (!apiKey && provider.hasKey) {
      await perform(async () => {
        apiKey = await proxyClient.revealProviderKey(provider.id);
        isRevealed = true;
      });
    } else {
      isRevealed = true;
    }
  }

  async function remove(): Promise<void> {
    if (
      !(await onConfirm(`Delete the saved ${name} key and endpoint? Active requests can finish.`))
    )
      return;
    await perform(async () => {
      const status = await proxyClient.deleteProvider(provider.id);
      apiKey = '';
      isRevealed = false;
      isDirty = false;
      onStatus(status);
      onNotice(`${name} key and configuration deleted.`);
    });
  }
</script>

<section class="provider" aria-labelledby={`heading-${provider.id}`}>
  <div class="provider-heading">
    <div class="provider-title">
      <span class="provider-symbol" aria-hidden="true">{provider.id === 'claude' ? 'C' : 'O'}</span>
      <h2 id={`heading-${provider.id}`}>{name}</h2>
    </div>
    <span class:configured={provider.hasKey} class="provider-state">
      {provider.credentialState === 'locked'
        ? 'Key unavailable'
        : provider.hasKey
          ? 'Key saved'
          : 'Not configured'}
    </span>
  </div>

  <form onsubmit={save}>
    <label for={`endpoint-${provider.id}`}>Upstream base URL</label>
    <input
      id={`endpoint-${provider.id}`}
      type="url"
      bind:value={endpoint}
      oninput={() => (isDirty = true)}
      disabled={locked}
      required
      spellcheck="false"
      autocomplete="off"
      aria-describedby={`endpoint-hint-${provider.id}`}
    />
    <p class="field-hint" id={`endpoint-hint-${provider.id}`}>
      {provider.id === 'openai'
        ? 'Include /v1, or your gateway’s API base path.'
        : 'Use the service root. The client adds /v1/messages.'}
    </p>

    <label for={`key-${provider.id}`}>API key</label>
    <div class="input-action">
      <input
        id={`key-${provider.id}`}
        type={isRevealed ? 'text' : 'password'}
        bind:value={apiKey}
        disabled={locked}
        placeholder={provider.hasKey
          ? 'Saved securely. Enter a key to replace it.'
          : 'Enter your API key'}
        autocomplete="new-password"
        spellcheck="false"
        required={!provider.hasKey}
      />
      <button
        type="button"
        class="input-button"
        onclick={reveal}
        disabled={locked}
        aria-label={`${isRevealed ? 'Hide' : 'Reveal'} ${name} API key`}
      >
        {isRevealed ? 'Hide' : 'Reveal'}
      </button>
    </div>
    <p class="field-hint">Stored in your operating system’s credential store.</p>

    <div class="provider-actions">
      <button type="submit" class="button secondary" disabled={locked}
        >{busy ? 'Working…' : 'Save settings'}</button
      >
      <button
        type="button"
        class="button quiet"
        onclick={test}
        disabled={locked || (!apiKey && !provider.hasKey)}>Test connection</button
      >
      {#if provider.hasKey}
        <button type="button" class="text-button danger" onclick={remove} disabled={locked}
          >Delete</button
        >
      {/if}
    </div>
  </form>
</section>
