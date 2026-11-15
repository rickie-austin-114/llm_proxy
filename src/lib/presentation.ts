import type { AppStatus, ProviderId } from './types';

export function providerName(id: ProviderId): string {
  return id === 'claude' ? 'Claude' : 'OpenAI';
}

export function errorMessage(error: unknown): string {
  if (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    typeof error.code === 'string' &&
    'message' in error &&
    typeof error.message === 'string'
  ) {
    return error.message;
  }
  return 'The operation could not be completed. Try again or reopen the app.';
}

export function parsePort(value: string): number | null {
  if (!/^\d+$/.test(value)) return null;
  const port = Number(value);
  return Number.isInteger(port) && port >= 1024 && port <= 65535 ? port : null;
}

export function previewStatus(): AppStatus {
  return {
    phase: 'stopped',
    localPort: 8787,
    httpsPort: 8788,
    httpsEnabled: false,
    certificate: { configured: false, trusted: false, expiresAt: null, fingerprint: null },
    startAtLogin: true,
    browserOrigins: ['https://pivot.claude.ai'],
    providers: [
      {
        id: 'claude',
        upstreamBaseUrl: 'https://api.servicesessentials.ibm.com',
        localBaseUrl: 'http://127.0.0.1:8787/claude',
        httpsBaseUrl: null,
        hasKey: false,
        credentialState: 'missing',
        lastTest: null,
      },
      {
        id: 'openai',
        upstreamBaseUrl: 'https://api.servicesessentials.ibm.com/v1',
        localBaseUrl: 'http://127.0.0.1:8787/openai/v1',
        httpsBaseUrl: null,
        hasKey: false,
        credentialState: 'missing',
        lastTest: null,
      },
    ],
    lastError: null,
    diagnostics: [],
    settingsNeedReset: false,
  };
}
