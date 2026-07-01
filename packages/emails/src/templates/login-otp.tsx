import { Section, Text } from "@react-email/components";
import type { CSSProperties } from "react";
import { EmailHeading, EmailShell, EmailText } from "./base";

export type LoginOtpType = "sign-in" | "email-verification" | "forget-password" | "change-email";

export type LoginOtpEmailProps = {
  otp: string;
  type: LoginOtpType;
  expiresInMinutes?: number;
};

type Copy = {
  preview: string;
  heading: string;
  message: string;
};

const COPY: Record<LoginOtpType, Copy> = {
  "sign-in": {
    preview: "Your Basse sign-in code",
    heading: "Sign in to Basse",
    message: "Enter this code to finish signing in. It works once and expires shortly.",
  },
  "email-verification": {
    preview: "Verify your Basse email",
    heading: "Verify your email",
    message: "Enter this code to confirm your email address and activate your account.",
  },
  "forget-password": {
    preview: "Reset your Basse password",
    heading: "Reset your password",
    message: "Enter this code to choose a new password for your account.",
  },
  "change-email": {
    preview: "Confirm your new Basse email",
    heading: "Confirm your email change",
    message: "Enter this code to confirm your new email address for your account.",
  },
};

export function LoginOtpEmail({ otp, type, expiresInMinutes = 5 }: LoginOtpEmailProps) {
  const copy = COPY[type];

  return (
    <EmailShell
      preview={copy.preview}
      accent="#fafafa"
      footnote="This is an automated security email from Basse. Never share this code with anyone."
    >
      <Section style={content}>
        <Text style={eyebrow}>One-time code</Text>
        <EmailHeading>{copy.heading}</EmailHeading>
        <EmailText>{copy.message}</EmailText>

        <Section style={codeBox}>
          <Text style={codeText}>{otp}</Text>
        </Section>

        <Text style={hint}>
          This code expires in {expiresInMinutes} minute{expiresInMinutes === 1 ? "" : "s"}. If you
          didn&rsquo;t request it, you can safely ignore this email.
        </Text>
      </Section>
    </EmailShell>
  );
}

const mono =
  '"Geist Mono", ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace';

const content: CSSProperties = {
  padding: "28px 28px 30px",
};

const eyebrow: CSSProperties = {
  color: "#737373",
  fontSize: "12px",
  fontWeight: "700",
  letterSpacing: "0.06em",
  lineHeight: "16px",
  textTransform: "uppercase",
  margin: "0",
};

const codeBox: CSSProperties = {
  backgroundColor: "#0f0f0f",
  border: "1px solid #262626",
  borderRadius: "12px",
  margin: "22px 0 20px",
  padding: "20px",
  textAlign: "center",
};

const codeText: CSSProperties = {
  color: "#fafafa",
  fontFamily: mono,
  fontSize: "32px",
  fontWeight: "600",
  letterSpacing: "0.42em",
  lineHeight: "40px",
  margin: "0",
  // Letter-spacing pads the right edge; nudge the block back to optical center.
  paddingLeft: "0.42em",
};

const hint: CSSProperties = {
  color: "#737373",
  fontSize: "13px",
  lineHeight: "20px",
  margin: "0",
};
