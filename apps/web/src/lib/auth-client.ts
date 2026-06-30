import { emailOTPClient, organizationClient } from "better-auth/client/plugins";
import { createAuthClient } from "better-auth/react";

// The inferred type of `authClient` references these plugin types. Re-exporting
// them keeps the generated declaration nameable under the composite build (TS2883).
import type { AuthQueryAtom } from "better-auth/client";
import type { AccessControl, Role } from "better-auth/plugins/access";
import type { OrganizationPlugin } from "better-auth/plugins/organization";
// The email-OTP client plugin's endpoints carry zod input schemas, so the inferred
// type reaches into zod's `$strip`; re-export it for the same reason as above.
import type { $strip } from "zod/v4/core";

export type { $strip, AccessControl, AuthQueryAtom, OrganizationPlugin, Role };

export const authClient = createAuthClient({
  baseURL: import.meta.env.VITE_API_URL ?? "",
  plugins: [emailOTPClient(), organizationClient()],
});

export type Session = typeof authClient.$Infer.Session;
export type Organization = typeof authClient.$Infer.Organization;
