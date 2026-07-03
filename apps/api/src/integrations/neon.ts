import { db, neonConnection } from "@basse/db";
import type { NeonBranch, NeonRegion } from "@basse/shared";
import { eq } from "drizzle-orm";
import { decryptSecret } from "../lib/crypto";

const NEON_API = "https://console.neon.tech/api/v2";

type NeonRegionResponse = {
  region_id: string;
  name: string;
};

type NeonBranchResponse = {
  id: string;
  name: string;
  default?: boolean;
  current_state?: string;
  created_at: string;
};

type NeonCreateProjectResponse = {
  project: { id: string; region_id: string };
  connection_uris: { connection_uri: string }[];
};

async function neonRequest(apiKey: string, path: string, init?: RequestInit): Promise<Response> {
  return fetch(`${NEON_API}${path}`, {
    ...init,
    headers: {
      accept: "application/json",
      authorization: `Bearer ${apiKey}`,
      ...(init?.body ? { "content-type": "application/json" } : {}),
      ...init?.headers,
    },
  });
}

async function neonError(response: Response, fallback: string): Promise<string> {
  const body = (await response.json().catch(() => null)) as { message?: string } | null;
  return body?.message || `${fallback} (${response.status})`;
}

// The /regions endpoint rejects organization API keys ("not allowed for
// organization API keys"), so keys are validated against /projects — which
// works for both personal and org keys — and region listing falls back to this
// static set when the live endpoint is unavailable (mirrors Neon's own CLI).
const FALLBACK_REGIONS: NeonRegion[] = [
  { id: "aws-us-east-1", name: "AWS US East (N. Virginia)" },
  { id: "aws-us-east-2", name: "AWS US East (Ohio)" },
  { id: "aws-us-west-2", name: "AWS US West (Oregon)" },
  { id: "aws-eu-central-1", name: "AWS Europe (Frankfurt)" },
  { id: "aws-eu-west-2", name: "AWS Europe (London)" },
  { id: "aws-ap-southeast-1", name: "AWS Asia Pacific (Singapore)" },
  { id: "aws-ap-southeast-2", name: "AWS Asia Pacific (Sydney)" },
  { id: "aws-sa-east-1", name: "AWS South America (São Paulo)" },
  { id: "azure-eastus2", name: "Azure East US 2 (Virginia)" },
  { id: "azure-westus3", name: "Azure West US 3 (Arizona)" },
  { id: "azure-gwc", name: "Azure Germany West Central (Frankfurt)" },
];

export async function validateNeonApiKey(apiKey: string): Promise<void> {
  const response = await neonRequest(apiKey, "/projects?limit=1");
  if (!response.ok) {
    throw new Error(await neonError(response, "Neon rejected the API key"));
  }
}

export async function listNeonRegions(apiKey: string): Promise<NeonRegion[]> {
  const response = await neonRequest(apiKey, "/regions");
  if (!response.ok) {
    return FALLBACK_REGIONS;
  }
  const body = (await response.json().catch(() => null)) as {
    regions?: NeonRegionResponse[];
  } | null;
  const regions = (body?.regions ?? []).map((region) => ({
    id: region.region_id,
    name: region.name,
  }));
  return regions.length > 0 ? regions : FALLBACK_REGIONS;
}

export async function createNeonProject(
  apiKey: string,
  input: { name: string; regionId: string },
): Promise<{ projectId: string; regionId: string; connectionUri: string }> {
  const response = await neonRequest(apiKey, "/projects", {
    method: "POST",
    body: JSON.stringify({ project: { name: input.name, region_id: input.regionId } }),
  });
  if (!response.ok) {
    throw new Error(await neonError(response, "Could not create the Neon project"));
  }
  const body = (await response.json()) as NeonCreateProjectResponse;
  const connectionUri = body.connection_uris[0]?.connection_uri;
  if (!connectionUri) {
    throw new Error("Neon did not return a connection string for the new project");
  }
  return {
    projectId: body.project.id,
    regionId: body.project.region_id,
    connectionUri,
  };
}

export async function deleteNeonProject(apiKey: string, projectId: string): Promise<void> {
  const response = await neonRequest(apiKey, `/projects/${encodeURIComponent(projectId)}`, {
    method: "DELETE",
  });
  // Already gone on Neon's side is fine — the goal is that it no longer exists.
  if (!response.ok && response.status !== 404) {
    throw new Error(await neonError(response, "Could not delete the Neon project"));
  }
}

function toBranch(branch: NeonBranchResponse): NeonBranch {
  return {
    id: branch.id,
    name: branch.name,
    isDefault: branch.default === true,
    currentState: branch.current_state ?? null,
    createdAt: branch.created_at,
  };
}

export async function listNeonBranches(apiKey: string, projectId: string): Promise<NeonBranch[]> {
  const response = await neonRequest(apiKey, `/projects/${encodeURIComponent(projectId)}/branches`);
  if (!response.ok) {
    throw new Error(await neonError(response, "Could not list Neon branches"));
  }
  const body = (await response.json()) as { branches?: NeonBranchResponse[] };
  return (body.branches ?? []).map(toBranch);
}

export async function createNeonBranch(
  apiKey: string,
  projectId: string,
  name: string,
): Promise<NeonBranch> {
  const response = await neonRequest(
    apiKey,
    `/projects/${encodeURIComponent(projectId)}/branches`,
    {
      method: "POST",
      // A read_write endpoint gives the branch its own compute so it is
      // connectable immediately.
      body: JSON.stringify({ branch: { name }, endpoints: [{ type: "read_write" }] }),
    },
  );
  if (!response.ok) {
    throw new Error(await neonError(response, "Could not create the Neon branch"));
  }
  const body = (await response.json()) as { branch: NeonBranchResponse };
  return toBranch(body.branch);
}

export async function deleteNeonBranch(
  apiKey: string,
  projectId: string,
  branchId: string,
): Promise<void> {
  const response = await neonRequest(
    apiKey,
    `/projects/${encodeURIComponent(projectId)}/branches/${encodeURIComponent(branchId)}`,
    { method: "DELETE" },
  );
  if (!response.ok && response.status !== 404) {
    throw new Error(await neonError(response, "Could not delete the Neon branch"));
  }
}

async function neonConnectionUri(
  apiKey: string,
  projectId: string,
  params: { branchId: string; database: string; role: string; pooled: boolean },
): Promise<string> {
  const query = new URLSearchParams({
    branch_id: params.branchId,
    database_name: params.database,
    role_name: params.role,
    pooled: String(params.pooled),
  });
  const response = await neonRequest(
    apiKey,
    `/projects/${encodeURIComponent(projectId)}/connection_uri?${query}`,
  );
  if (!response.ok) {
    throw new Error(await neonError(response, "Could not fetch the Neon connection string"));
  }
  const body = (await response.json()) as { uri?: string };
  if (!body.uri) {
    throw new Error("Neon did not return a connection string");
  }
  return body.uri;
}

export async function getNeonBranchConnection(
  apiKey: string,
  projectId: string,
  branchId: string,
): Promise<{ pooledUri: string; directUri: string; database: string; role: string }> {
  const response = await neonRequest(
    apiKey,
    `/projects/${encodeURIComponent(projectId)}/branches/${encodeURIComponent(branchId)}/databases`,
  );
  if (!response.ok) {
    throw new Error(await neonError(response, "Could not list the branch's databases"));
  }
  const body = (await response.json()) as {
    databases?: { name: string; owner_name: string }[];
  };
  const database = body.databases?.[0];
  if (!database) {
    throw new Error("The branch has no databases yet");
  }

  const params = { branchId, database: database.name, role: database.owner_name };
  const [pooledUri, directUri] = await Promise.all([
    neonConnectionUri(apiKey, projectId, { ...params, pooled: true }),
    neonConnectionUri(apiKey, projectId, { ...params, pooled: false }),
  ]);
  return { pooledUri, directUri, database: database.name, role: database.owner_name };
}

/** The workspace's decrypted Neon API key, or null when not connected. */
export async function getNeonApiKey(organizationId: string): Promise<string | null> {
  const [connection] = await db
    .select()
    .from(neonConnection)
    .where(eq(neonConnection.organizationId, organizationId))
    .limit(1);
  if (!connection) return null;
  return decryptSecret(connection.apiKey);
}
