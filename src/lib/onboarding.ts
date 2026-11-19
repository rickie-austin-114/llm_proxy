import type { AppStatus, ProviderDraft, ProviderId } from './types';

export type SetupStep = 1 | 2 | 3;

export function httpsReady(status: AppStatus, nowMs = Date.now()): boolean {
  return (
    status.httpsEnabled &&
    status.certificate.configured &&
    status.certificate.trusted &&
    status.certificate.expiresAt !== null &&
    status.certificate.expiresAt * 1000 > nowMs
  );
}

export function setupStep(status: AppStatus, nowMs = Date.now()): SetupStep {
  if (!status.providers.some((provider) => provider.hasKey)) return 1;
  return httpsReady(status, nowMs) ? 3 : 2;
}

interface SkipSetupClient {
  saveProvider: (id: ProviderId, endpoint: string, apiKey: string | null) => Promise<AppStatus>;
  skipOnboarding: (draft: ProviderDraft | null) => Promise<AppStatus>;
}

export async function skipOnboarding(
  status: AppStatus,
  input: ProviderDraft & { apiKey: string },
  client: SkipSetupClient,
  onProviderSaved: (status: AppStatus) => void,
): Promise<AppStatus> {
  const provider = status.providers.find((item) => item.id === input.id)!;
  const endpoint = input.upstreamBaseUrl.trim();
  const key = input.apiKey.trim();
  const endpointChanged = endpoint !== provider.upstreamBaseUrl;
  let draft: ProviderDraft | null = null;
  if (key || (endpointChanged && provider.hasKey)) {
    // Persist secrets through the credential command before dismissing the wizard.
    // Clear the input as soon as it is saved, even if the following dismissal fails.
    onProviderSaved(await client.saveProvider(input.id, endpoint, key || null));
  } else if (endpointChanged) {
    draft = { id: input.id, upstreamBaseUrl: endpoint };
  }
  return client.skipOnboarding(draft);
}

interface HttpsSetupClient {
  setupHttps: () => Promise<AppStatus>;
  trustHttpsCertificate: () => Promise<AppStatus>;
  checkHttpsTrust: () => Promise<AppStatus>;
}

export async function setupOnboardingHttps(
  status: AppStatus,
  client: HttpsSetupClient,
): Promise<AppStatus> {
  // Reuse a valid identity after a canceled OS trust prompt.
  const reusable =
    status.httpsEnabled &&
    status.certificate.configured &&
    status.certificate.expiresAt !== null &&
    status.certificate.expiresAt * 1000 > Date.now();
  if (!httpsReady(status)) {
    await (reusable ? client.trustHttpsCertificate() : client.setupHttps());
  }
  return client.checkHttpsTrust();
}
