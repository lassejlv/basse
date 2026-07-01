export type ApiHealth = {
  ok: boolean;
  service: string;
};

/**
 * Stable per-tab id sent with mutations (x-basse-client) and the realtime
 * socket (?client=), so the server can skip echoing a client's own events
 * back to it — the mutation response already carried the fresh state.
 */
export const apiClientId = crypto.randomUUID();

export const apiClientHeader = { "x-basse-client": apiClientId } as const;

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

export async function fetchApiHealth(): Promise<ApiHealth> {
  const response = await fetch(`${apiBaseUrl}/health`);

  if (!response.ok) {
    throw new Error(`API health request failed with ${response.status}`);
  }

  return response.json() as Promise<ApiHealth>;
}
