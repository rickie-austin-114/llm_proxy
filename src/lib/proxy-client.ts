import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AppStatus, ProviderDraft, ProviderId } from './types';

export const isDesktop = isTauri;

export const proxyClient = {
  finishOnboarding: (): Promise<AppStatus> => invoke('finish_onboarding'),
  skipOnboarding: (draft: ProviderDraft | null): Promise<AppStatus> =>
    invoke('skip_onboarding', { draft }),
  getStatus: (): Promise<AppStatus> => invoke('get_status'),
  checkBackgroundStartup: (): Promise<AppStatus> => invoke('check_background_startup'),
  openStartupSettings: (): Promise<void> => invoke('open_startup_settings'),
  start: (): Promise<AppStatus> => invoke('start_proxy'),
  stop: (): Promise<AppStatus> => invoke('stop_proxy'),
  saveProvider: (
    id: ProviderId,
    upstreamBaseUrl: string,
    apiKey: string | null,
  ): Promise<AppStatus> => invoke('save_provider', { id, upstreamBaseUrl, apiKey }),
  deleteProvider: (id: ProviderId): Promise<AppStatus> => invoke('delete_provider', { id }),
  testConnection: (
    id: ProviderId,
    upstreamBaseUrl: string,
    apiKey: string | null,
  ): Promise<string> => invoke('test_connection', { id, upstreamBaseUrl, apiKey }),
  setPort: (port: number): Promise<AppStatus> => invoke('set_local_port', { port }),
  setHttpsPort: (port: number): Promise<AppStatus> => invoke('set_https_port', { port }),
  setupHttps: (): Promise<AppStatus> => invoke('setup_https'),
  trustHttpsCertificate: (): Promise<AppStatus> => invoke('trust_https_certificate'),
  checkHttpsTrust: (): Promise<AppStatus> => invoke('check_https_trust'),
  removeHttps: (): Promise<AppStatus> => invoke('remove_https'),
  setBrowserOrigins: (origins: string[]): Promise<AppStatus> =>
    invoke('set_browser_origins', { origins }),
  revealProviderKey: (id: ProviderId): Promise<string> => invoke('reveal_provider_key', { id }),
  checkSavedKeyAccess: (): Promise<void> => invoke('check_saved_key_access'),
  copyText: (value: string): Promise<void> => invoke('copy_text', { value }),
  reset: (): Promise<AppStatus> => invoke('reset_app_data'),
  onStatus: (callback: (status: AppStatus) => void): Promise<UnlistenFn> =>
    listen<AppStatus>('app-status', (event) => callback(event.payload)),
};
