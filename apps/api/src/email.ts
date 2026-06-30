import { db, member, user } from "@basse/db";
import { type LoginOtpType, renderLoginOtpEmail, renderMonitorAlertEmail } from "@basse/emails";
import { createEmailClient, type EmailClient } from "@opencoredev/email-sdk";
import { cloudflare } from "@opencoredev/email-sdk/cloudflare";
import { observabilityPlugin } from "@opencoredev/email-sdk/plugins/observability";
import { eq } from "drizzle-orm";

type AlertEmail = {
  id: string;
  organizationId: string;
  severity: "info" | "warning" | "critical";
  title: string;
  message: string;
  code: string;
  fingerprint: string;
};

let emailClient: EmailClient | null | undefined;
let warnedMissingConfig = false;

function appUrl(): string {
  return (Bun.env.BASSE_PUBLIC_URL ?? Bun.env.WEB_ORIGIN ?? Bun.env.BETTER_AUTH_URL ?? "").replace(
    /\/$/,
    "",
  );
}

function getEmailClient(): EmailClient | null {
  if (emailClient !== undefined) return emailClient;

  const apiToken = Bun.env.CLOUDFLARE_EMAIL_API_TOKEN ?? Bun.env.CLOUDFLARE_API_TOKEN;
  const accountId = Bun.env.CLOUDFLARE_ACCOUNT_ID;
  if (!apiToken || !accountId || !Bun.env.EMAIL_FROM) {
    emailClient = null;
    return emailClient;
  }

  emailClient = createEmailClient({
    adapters: [cloudflare({ apiToken, accountId })],
    defaultAdapter: "cloudflare",
    retry: { retries: 1 },
    plugins: [
      observabilityPlugin({
        log(event) {
          console.log("[email]", event.type, event.provider, `attempt=${event.attempt}`);
        },
      }),
    ],
  });
  return emailClient;
}

const OTP_SUBJECT: Record<LoginOtpType, string> = {
  "sign-in": "Your Basse sign-in code",
  "email-verification": "Verify your Basse email",
  "forget-password": "Reset your Basse password",
  "change-email": "Confirm your new Basse email",
};

export async function sendOtpEmail(params: {
  email: string;
  otp: string;
  type: LoginOtpType;
}): Promise<void> {
  const { email, otp, type } = params;
  const client = getEmailClient();

  if (!client) {
    // Without an email provider configured (e.g. local dev), fall back to logging
    // so the code is still retrievable and the OTP flow remains testable.
    console.warn(`[email] OTP delivery disabled; code for ${email} (${type}): ${otp}`);
    return;
  }

  const rendered = await renderLoginOtpEmail({ otp, type });
  await client.send(
    {
      from: Bun.env.EMAIL_FROM!,
      to: email,
      subject: OTP_SUBJECT[type],
      text: rendered.text,
      html: rendered.html,
    },
    {
      metadata: { kind: "otp", type },
    },
  );
}

async function organizationRecipients(organizationId: string): Promise<string[]> {
  const rows = await db
    .select({ email: user.email })
    .from(member)
    .innerJoin(user, eq(member.userId, user.id))
    .where(eq(member.organizationId, organizationId));

  return [...new Set(rows.map((row) => row.email).filter(Boolean))];
}

export async function sendAlertEmail(alert: AlertEmail): Promise<void> {
  const client = getEmailClient();
  if (!client) {
    if (!warnedMissingConfig) {
      warnedMissingConfig = true;
      console.warn("[email] alert email disabled; configure Cloudflare email env vars");
    }
    return;
  }

  const recipients = await organizationRecipients(alert.organizationId);
  if (recipients.length === 0) return;

  const baseUrl = appUrl();
  const alertsUrl = baseUrl ? `${baseUrl}/alerts` : null;
  const subject = `[Basse ${alert.severity}] ${alert.title}`;
  const rendered = await renderMonitorAlertEmail({
    title: alert.title,
    message: alert.message,
    severity: alert.severity,
    code: alert.code,
    alertsUrl,
  });

  await Promise.all(
    recipients.map((email) =>
      client
        .send(
          {
            from: Bun.env.EMAIL_FROM!,
            to: email,
            subject,
            text: rendered.text,
            html: rendered.html,
            headers: {
              "X-Basse-Alert-Code": alert.code,
            },
          },
          {
            idempotencyKey: `basse-alert:${alert.id}:${email}`,
            metadata: {
              alertId: alert.id,
              code: alert.code,
            },
          },
        )
        .catch((error) => {
          console.error(
            "[email] alert delivery failed",
            error instanceof Error ? error.message : error,
          );
        }),
    ),
  );
}
