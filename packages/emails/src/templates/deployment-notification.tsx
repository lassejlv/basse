import { Button, Column, Row, Section, Text } from "@react-email/components";
import type { CSSProperties } from "react";
import { EmailHeading, EmailShell, EmailText } from "./base";

export type DeploymentNotificationEmailProps = {
  appName: string;
  projectName: string;
  environmentName: string;
  deploymentId: string;
  status: "healthy" | "failed";
  commitSha?: string | null;
  deploymentUrl?: string | null;
};

const STATUS = {
  healthy: { label: "Deployment succeeded", accent: "#22c55e" },
  failed: { label: "Deployment failed", accent: "#ef4444" },
} satisfies Record<DeploymentNotificationEmailProps["status"], { label: string; accent: string }>;

export function DeploymentNotificationEmail({
  appName,
  projectName,
  environmentName,
  deploymentId,
  status,
  commitSha,
  deploymentUrl,
}: DeploymentNotificationEmailProps) {
  const tone = STATUS[status];
  const shortSha = commitSha?.slice(0, 7) ?? "not recorded";

  return (
    <EmailShell
      accent={tone.accent}
      footnote="You’re receiving this because deploy notifications are enabled for this Basse app."
      preview={`${tone.label} · ${appName}`}
    >
      <Section style={content}>
        <Row style={eyebrowRow}>
          <Column style={dotCell}>
            <span style={{ ...dot, backgroundColor: tone.accent }} />
          </Column>
          <Column>
            <Text style={{ ...eyebrow, color: tone.accent }}>{tone.label}</Text>
          </Column>
        </Row>

        <EmailHeading>{appName}</EmailHeading>
        <EmailText>
          {projectName} / {environmentName} finished with status {status}.
        </EmailText>

        <Section style={detail}>
          <Text style={detailLabel}>Deployment</Text>
          <Text style={detailValue}>{deploymentId}</Text>
          <Text style={detailLabel}>Commit</Text>
          <Text style={detailValue}>{shortSha}</Text>
        </Section>

        {deploymentUrl ? (
          <Section style={buttonRow}>
            <Button href={deploymentUrl} style={button}>
              Open deployment &rarr;
            </Button>
          </Section>
        ) : null}
      </Section>
    </EmailShell>
  );
}

const mono =
  '"Geist Mono", ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace';

const content: CSSProperties = {
  padding: "28px 28px 30px",
};

const eyebrowRow: CSSProperties = {
  marginBottom: "2px",
};

const dotCell: CSSProperties = {
  width: "14px",
  verticalAlign: "middle",
};

const dot: CSSProperties = {
  display: "inline-block",
  width: "8px",
  height: "8px",
  borderRadius: "9999px",
};

const eyebrow: CSSProperties = {
  fontSize: "12px",
  fontWeight: "700",
  letterSpacing: "0.06em",
  lineHeight: "16px",
  textTransform: "uppercase",
  margin: "0",
};

const detail: CSSProperties = {
  backgroundColor: "#0f0f0f",
  border: "1px solid #262626",
  borderRadius: "10px",
  margin: "22px 0 24px",
  padding: "14px 16px",
};

const detailLabel: CSSProperties = {
  color: "#737373",
  fontSize: "11px",
  fontWeight: "600",
  letterSpacing: "0.06em",
  lineHeight: "16px",
  textTransform: "uppercase",
  margin: "0 0 6px",
};

const detailValue: CSSProperties = {
  color: "#fafafa",
  fontFamily: mono,
  fontSize: "13px",
  lineHeight: "20px",
  margin: "0 0 12px",
};

const buttonRow: CSSProperties = {
  marginTop: "2px",
};

const button: CSSProperties = {
  backgroundColor: "#fafafa",
  borderRadius: "8px",
  color: "#0a0a0a",
  display: "inline-block",
  fontSize: "14px",
  fontWeight: "600",
  lineHeight: "20px",
  padding: "11px 18px",
  textDecoration: "none",
};
