import { Button, Column, Row, Section, Text } from "@react-email/components";
import type { CSSProperties } from "react";
import { EmailHeading, EmailShell, EmailText } from "./base";

export type MonitorAlertEmailProps = {
  title: string;
  message: string;
  severity: "info" | "warning" | "critical";
  code: string;
  alertsUrl?: string | null;
};

type SeverityStyle = {
  label: string;
  accent: string;
};

const SEVERITY: Record<MonitorAlertEmailProps["severity"], SeverityStyle> = {
  info: { label: "Information", accent: "#3b82f6" },
  warning: { label: "Warning", accent: "#f59e0b" },
  critical: { label: "Critical", accent: "#ef4444" },
};

export function MonitorAlertEmail({
  title,
  message,
  severity,
  code,
  alertsUrl,
}: MonitorAlertEmailProps) {
  const tone = SEVERITY[severity];
  const preview = `${tone.label} alert · ${title}`;

  return (
    <EmailShell preview={preview} accent={tone.accent}>
      <Section style={content}>
        <Row style={eyebrowRow}>
          <Column style={dotCell}>
            <span style={{ ...dot, backgroundColor: tone.accent }} />
          </Column>
          <Column style={eyebrowCell}>
            <Text style={{ ...eyebrow, color: tone.accent }}>{tone.label} alert</Text>
          </Column>
        </Row>

        <EmailHeading>{title}</EmailHeading>
        <EmailText>{message}</EmailText>

        <Section style={detail}>
          <Text style={detailLabel}>Alert code</Text>
          <Text style={detailValue}>{code}</Text>
        </Section>

        {alertsUrl ? (
          <Section style={buttonRow}>
            <Button href={alertsUrl} style={button}>
              Open alerts &rarr;
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

const eyebrowCell: CSSProperties = {
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
  margin: "0",
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
