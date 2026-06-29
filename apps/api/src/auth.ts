import { db, ensurePersonalWorkspace } from "@basse/db";
import * as schema from "@basse/db/schema";
import { betterAuth } from "better-auth";
import { drizzleAdapter } from "better-auth/adapters/drizzle";
import { organization } from "better-auth/plugins";

const trustedOrigins = [
  "http://localhost:5173",
  "http://127.0.0.1:5173",
  Bun.env.WEB_ORIGIN,
].filter((origin): origin is string => Boolean(origin));

export const auth = betterAuth({
  database: drizzleAdapter(db, {
    provider: "pg",
    schema,
  }),
  databaseHooks: {
    user: {
      create: {
        after: async (user) => {
          await ensurePersonalWorkspace(user);
        },
      },
    },
  },
  emailAndPassword: {
    enabled: true,
  },
  plugins: [organization()],
  secret: Bun.env.BETTER_AUTH_SECRET,
  baseURL: Bun.env.BETTER_AUTH_URL ?? "http://localhost:3000",
  trustedOrigins,
});
