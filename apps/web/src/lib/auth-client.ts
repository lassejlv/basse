import { organizationClient } from "better-auth/client/plugins";
import { createAuthClient } from "better-auth/react";

// The inferred type of `authClient` references these plugin types. Re-exporting
// them keeps the generated declaration nameable under the composite build (TS2883).
import type { AuthQueryAtom } from "better-auth/client";
import type { AccessControl, Role } from "better-auth/plugins/access";
import type { OrganizationPlugin } from "better-auth/plugins/organization";

export type { AccessControl, AuthQueryAtom, OrganizationPlugin, Role };

export const authClient = createAuthClient({
  baseURL: import.meta.env.VITE_API_URL ?? "",
  plugins: [organizationClient()],
});

export type Session = typeof authClient.$Infer.Session;
export type Organization = typeof authClient.$Infer.Organization;
