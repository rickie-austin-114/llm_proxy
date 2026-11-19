import { describe, expect, it, vi } from 'vitest';
import { httpsReady, setupOnboardingHttps, setupStep, skipOnboarding } from './onboarding';
import { previewStatus } from './presentation';

describe('resuming first-run setup', () => {
  it('requires one saved provider and currently trusted HTTPS before the final step', () => {
    const status = previewStatus();
    expect(setupStep(status)).toBe(1);
    status.providers[1]!.hasKey = true;
    expect(setupStep(status)).toBe(2);
    status.httpsEnabled = true;
    status.certificate = {
      configured: true,
      trusted: false,
      expiresAt: 2000,
      fingerprint: 'fixture',
    };
    expect(setupStep(status, 1000_000)).toBe(2);
    status.certificate.trusted = true;
    expect(setupStep(status, 1000_000)).toBe(3);
    expect(setupStep(status, 2000_000)).toBe(2);
    status.certificate.expiresAt = null;
    expect(httpsReady(status, 1000_000)).toBe(false);
    status.providers[1]!.hasKey = false;
    expect(setupStep(status)).toBe(1);
  });

  it('reuses the saved certificate after trust was canceled and checks native trust', async () => {
    const status = previewStatus();
    status.httpsEnabled = true;
    status.certificate = {
      configured: true,
      trusted: false,
      expiresAt: Date.now() / 1000 + 3600,
      fingerprint: 'same-certificate',
    };
    const client = {
      setupHttps: vi.fn(),
      trustHttpsCertificate: vi
        .fn()
        .mockRejectedValueOnce({ code: 'trust_denied' })
        .mockResolvedValue(status),
      checkHttpsTrust: vi
        .fn()
        .mockResolvedValue({ ...status, certificate: { ...status.certificate, trusted: true } }),
    };
    await expect(setupOnboardingHttps(status, client)).rejects.toEqual({ code: 'trust_denied' });
    expect(client.checkHttpsTrust).not.toHaveBeenCalled();
    expect(httpsReady(await setupOnboardingHttps(status, client))).toBe(true);
    expect(client.setupHttps).not.toHaveBeenCalled();
    expect(client.trustHttpsCertificate).toHaveBeenCalledTimes(2);
  });

  it('creates an identity for new or expired HTTPS and never assumes creation establishes trust', async () => {
    const status = previewStatus();
    const client = {
      setupHttps: vi.fn().mockResolvedValue(status),
      trustHttpsCertificate: vi.fn(),
      checkHttpsTrust: vi.fn().mockResolvedValue(status),
    };
    expect(httpsReady(await setupOnboardingHttps(status, client))).toBe(false);
    status.httpsEnabled = true;
    status.certificate = { configured: true, trusted: true, expiresAt: 1, fingerprint: 'expired' };
    await setupOnboardingHttps(status, client);
    expect(client.setupHttps).toHaveBeenCalledTimes(2);
    expect(client.trustHttpsCertificate).not.toHaveBeenCalled();
    expect(client.checkHttpsTrust).toHaveBeenCalledTimes(2);
  });
});

describe('skipping setup preserves entered details', () => {
  function client() {
    return {
      saveProvider: vi.fn().mockResolvedValue(previewStatus()),
      skipOnboarding: vi.fn().mockResolvedValue({ ...previewStatus(), onboardingSkipped: true }),
    };
  }

  it('skips an untouched wizard without requiring a key or saving a default endpoint', async () => {
    const status = previewStatus();
    const api = client();
    const saved = vi.fn();
    const next = await skipOnboarding(status, { ...status.providers[0]!, apiKey: '' }, api, saved);
    expect(next.onboardingSkipped).toBe(true);
    expect(api.saveProvider).not.toHaveBeenCalled();
    expect(api.skipOnboarding).toHaveBeenCalledWith(null);
    expect(saved).not.toHaveBeenCalled();
  });

  it('securely saves a typed key and endpoint before dismissing the wizard', async () => {
    const status = previewStatus();
    const api = client();
    const saved = vi.fn();
    await skipOnboarding(
      status,
      { id: 'openai', upstreamBaseUrl: ' https://example.com/v1 ', apiKey: ' fake-key ' },
      api,
      saved,
    );
    expect(api.saveProvider).toHaveBeenCalledWith('openai', 'https://example.com/v1', 'fake-key');
    expect(saved).toHaveBeenCalledOnce();
    expect(api.saveProvider.mock.invocationCallOrder[0]).toBeLessThan(
      saved.mock.invocationCallOrder[0]!,
    );
    expect(saved.mock.invocationCallOrder[0]).toBeLessThan(
      api.skipOnboarding.mock.invocationCallOrder[0]!,
    );
    expect(api.skipOnboarding).toHaveBeenCalledWith(null);
  });

  it('keeps an endpoint-only edit without creating a fake saved key', async () => {
    const status = previewStatus();
    const api = client();
    await skipOnboarding(
      status,
      { id: 'claude', upstreamBaseUrl: 'https://example.com/gateway', apiKey: '' },
      api,
      vi.fn(),
    );
    expect(api.saveProvider).not.toHaveBeenCalled();
    expect(api.skipOnboarding).toHaveBeenCalledWith({
      id: 'claude',
      upstreamBaseUrl: 'https://example.com/gateway',
    });
  });

  it('updates an existing provider endpoint while retaining its saved key', async () => {
    const status = previewStatus();
    status.providers[0]!.hasKey = true;
    const api = client();
    await skipOnboarding(
      status,
      { id: 'claude', upstreamBaseUrl: 'https://example.com/new', apiKey: '' },
      api,
      vi.fn(),
    );
    expect(api.saveProvider).toHaveBeenCalledWith('claude', 'https://example.com/new', null);
    expect(api.skipOnboarding).toHaveBeenCalledWith(null);
  });

  it('does not dismiss setup or clear unsaved input after a credential-save failure', async () => {
    const status = previewStatus();
    const api = client();
    const saved = vi.fn();
    const input = {
      id: 'claude' as const,
      upstreamBaseUrl: 'https://example.com',
      apiKey: 'fake-key',
    };
    api.saveProvider.mockRejectedValue({ code: 'credential_store_unavailable' });
    await expect(skipOnboarding(status, input, api, saved)).rejects.toEqual({
      code: 'credential_store_unavailable',
    });
    expect(api.skipOnboarding).not.toHaveBeenCalled();
    expect(saved).not.toHaveBeenCalled();
    expect(input.apiKey).toBe('fake-key');
  });

  it('acknowledges a saved key even if persisting dismissal subsequently fails', async () => {
    const status = previewStatus();
    const api = client();
    const saved = vi.fn();
    api.skipOnboarding.mockRejectedValue({ code: 'settings_save_failed' });
    await expect(
      skipOnboarding(status, { ...status.providers[0]!, apiKey: 'fake-key' }, api, saved),
    ).rejects.toEqual({ code: 'settings_save_failed' });
    expect(saved).toHaveBeenCalledOnce();
  });
});
