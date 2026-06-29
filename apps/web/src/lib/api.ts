export type ApiHealth = {
  ok: boolean;
  service: string;
};

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

export async function fetchApiHealth(): Promise<ApiHealth> {
  const response = await fetch(`${apiBaseUrl}/health`);

  if (!response.ok) {
    throw new Error(`API health request failed with ${response.status}`);
  }

  return response.json() as Promise<ApiHealth>;
}
