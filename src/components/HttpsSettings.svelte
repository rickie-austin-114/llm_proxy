<script lang="ts">
  import { proxyClient } from '../lib/proxy-client';
  import { parsePort } from '../lib/presentation';
  import type { AppStatus } from '../lib/types';

  interface Props {
    status: AppStatus;
    disabled: boolean;
    running: boolean;
    onStatus: (status: AppStatus) => void;
    onNotice: (message: string, isError?: boolean) => void;
    onConfirm: (message: string) => Promise<boolean>;
    perform: (action: () => Promise<void>) => Promise<void>;
  }

  let { status, disabled, running, onStatus, onNotice, onConfirm, perform }: Props = $props();
  let portInput = $derived(String(status.httpsPort));
  const expired = $derived(
    status.certificate.expiresAt !== null && status.certificate.expiresAt * 1000 <= Date.now(),
  );

  async function setup(): Promise<void> {
    if (
      status.certificate.configured &&
      !(await onConfirm(
        'Replace the local HTTPS certificate and remove its previous trust? You will need to trust the new certificate and restart Office.',
      ))
    )
      return;
    await perform(async () => {
      onStatus(await proxyClient.setupHttps());
      onNotice('HTTPS is set up. Select Trust certificate, then start the proxy.');
    });
  }

  async function trust(): Promise<void> {
    if (
      !(await onConfirm(
        'Trust llm_proxy’s local HTTPS certificate for this user account? This lets Office verify connections to localhost. The OS may ask for approval.',
      ))
    )
      return;
    await perform(async () => {
      onStatus(await proxyClient.trustHttpsCertificate());
      onNotice(
        'Certificate trust verified. Restart Office, start the proxy, and use the HTTPS URL below.',
      );
    });
  }

  async function remove(): Promise<void> {
    if (
      !(await onConfirm(
        'Disable HTTPS and remove its certificate trust and private key? Existing HTTP client settings are preserved.',
      ))
    )
      return;
    await perform(async () => {
      onStatus(await proxyClient.removeHttps());
      onNotice('HTTPS certificate and trust removed.');
    });
  }

  async function savePort(): Promise<void> {
    const port = parsePort(portInput);
    if (port === null) {
      onNotice('Choose a whole-number HTTPS port between 1024 and 65535.', true);
      return;
    }
    await perform(async () => {
      onStatus(await proxyClient.setHttpsPort(port));
      onNotice('HTTPS port saved. Update the URL in Office before reconnecting.');
    });
  }

  async function checkTrust(): Promise<void> {
    await perform(async () => {
      const next = await proxyClient.checkHttpsTrust();
      onStatus(next);
      onNotice(
        next.certificate.trusted
          ? 'Certificate trust verified. Restart Office if it was already open.'
          : 'Certificate trust is not verified. Trust the certificate or renew it if expired.',
        !next.certificate.trusted,
      );
    });
  }
</script>

<section class="https-settings" aria-labelledby="https-heading">
  <h2 id="https-heading">Office gateway HTTPS</h2>
  <p>Set up and trust a local certificate to connect Claude for Office over HTTPS.</p>
  <p class="certificate-state" role="status">
    {#if !status.certificate.configured}HTTPS is not set up.
    {:else if expired}Certificate expired. Stop the proxy and renew it.
    {:else if status.certificate.trusted}Certificate trusted on this computer.
    {:else}Certificate created. Trust it before connecting Office.{/if}
  </p>
  {#if status.certificate.expiresAt}
    <p>Certificate expires {new Date(status.certificate.expiresAt * 1000).toLocaleDateString()}.</p>
  {/if}
  <div class="https-port">
    <label for="https-port">HTTPS port</label>
    <input
      id="https-port"
      inputmode="numeric"
      bind:value={portInput}
      disabled={disabled || running}
    />
    <button
      class="button secondary small"
      onclick={savePort}
      disabled={disabled || running || portInput === String(status.httpsPort)}>Apply port</button
    >
  </div>
  <div class="https-actions">
    <button class="button secondary small" onclick={setup} disabled={disabled || running}>
      {status.certificate.configured ? 'Renew certificate' : 'Set up HTTPS'}
    </button>
    <button
      class="button secondary small"
      onclick={trust}
      disabled={disabled || !status.certificate.configured || expired}>Trust certificate</button
    >
    <button
      class="text-button"
      onclick={checkTrust}
      disabled={disabled || !status.certificate.configured}>Check certificate trust</button
    >
    {#if status.certificate.configured}
      <button class="text-button danger" onclick={remove} disabled={disabled || running}
        >Remove HTTPS</button
      >
    {/if}
  </div>
  <p class="field-hint">
    Stop the proxy to set up, renew, remove, or change its HTTPS port. After trusting the
    certificate, restart Office and use the HTTPS base URL below with your saved provider API key.
    Managed computers may require administrator approval.
  </p>
  {#if status.certificate.fingerprint}
    <details>
      <summary>Certificate fingerprint (SHA-256)</summary><code
        >{status.certificate.fingerprint}</code
      >
    </details>
  {/if}
</section>

<style>
  .https-settings {
    margin: 28px 0;
    padding: 22px;
    border: 1px solid #d8e0df;
    border-radius: 14px;
  }
  h2 {
    margin: 0 0 8px;
    font-size: 17px;
  }
  p {
    font-size: 13px;
    line-height: 1.6;
  }
  .certificate-state {
    font-weight: 600;
  }
  .https-port,
  .https-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    margin: 14px 0;
  }
  .https-port label {
    font-size: 13px;
  }
  .https-port input {
    width: 90px;
    padding: 8px;
    border: 1px solid #cbd5d2;
    border-radius: 6px;
  }
  details {
    font-size: 12px;
  }
  code {
    display: block;
    overflow-wrap: anywhere;
    margin-top: 8px;
  }
</style>
