import { db, ensurePersonalWorkspace } from "@basse/db";
import * as schema from "@basse/db/schema";
import { betterAuth } from "better-auth";
import { drizzleAdapter } from "better-auth/adapters/drizzle";
import { emailOTP, organization } from "better-auth/plugins";
import { sendOtpEmail } from "./email";

const trustedOrigins = [
  "http://localhost:5173",
  "http://127.0.0.1:5173",
  Bun.env.WEB_ORIGIN,
].filter((origin): origin is string => Boolean(origin));

const emailVerificationEnabled = Bun.env.EMAIL_VERIFICATION !== "false";

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
    requireEmailVerification: emailVerificationEnabled,
  },
  plugins: [
    emailOTP({
      // Verify email addresses with a one-time code instead of a magic link.
      overrideDefaultEmailVerification: true,
      async sendVerificationOTP({ email, otp, type }) {
        await sendOtpEmail({ email, otp, type });
      },
    }),
    organization(),
  ],
  secret: Bun.env.BETTER_AUTH_SECRET,
  baseURL: Bun.env.BETTER_AUTH_URL ?? "http://localhost:3000",
  trustedOrigins,
});
