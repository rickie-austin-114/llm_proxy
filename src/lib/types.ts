export type ProviderId = 'claude' | 'openai';
export type ProxyPhase = 'stopped' | 'starting' | 'running' | 'stopping' | 'error';

export interface AppError {
  code: string;
  message: string;
}

export interface ProviderStatus {
  id: ProviderId;
  upstreamBaseUrl: string;
  localBaseUrl: string;
  httpsBaseUrl: string | null;
  hasKey: boolean;
  credentialState: string;
  lastTest: string | null;
}

export interface Diagnostic {
  id: number;
  timestamp: number;
  code: string;
  message: string;
}

export interface AppStatus {
  runInBackground: boolean;
  onboardingComplete: boolean;
  onboardingSkipped: boolean;
  platform: string;
  phase: ProxyPhase;
  localPort: number;
  httpsPort: number;
  httpsEnabled: boolean;
  certificate: {
    configured: boolean;
    trusted: boolean;
    expiresAt: number | null;
    fingerprint: string | null;
  };
  backgroundStartup: string;
  browserOrigins: string[];
  providers: ProviderStatus[];
  lastError: AppError | null;
  diagnostics: Diagnostic[];
  settingsNeedReset: boolean;
}

export interface ProviderDraft {
  id: ProviderId;
  upstreamBaseUrl: string;
}
