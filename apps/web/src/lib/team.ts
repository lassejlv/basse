import type {
  InviteTeamMemberInput,
  TeamInvitation,
  TeamMember,
  TeamOverview,
  UpdateTeamMemberInput,
} from "@basse/shared";

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function getTeam(): Promise<TeamOverview> {
  const response = await fetch(`${apiBaseUrl}/api/team`, { credentials: "include" });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<TeamOverview>;
}

export async function inviteTeamMember(
  input: InviteTeamMemberInput,
): Promise<TeamMember | TeamInvitation> {
  const response = await fetch(`${apiBaseUrl}/api/team/invitations`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<TeamMember | TeamInvitation>;
}

export async function updateTeamMember(
  id: string,
  input: UpdateTeamMemberInput,
): Promise<TeamMember> {
  const response = await fetch(`${apiBaseUrl}/api/team/members/${id}`, {
    method: "PATCH",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<TeamMember>;
}

export async function deleteTeamMember(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/team/members/${id}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export async function deleteTeamInvitation(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/team/invitations/${id}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}
