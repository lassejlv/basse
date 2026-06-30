import { Button, Section, Text } from "@react-email/components";
import { EmailHeading, EmailShell, EmailText } from "./base";

export type MonitorAlertEmailProps = {
  title: string;
  message: string;
  severity: "info" | "warning" | "critical";
  code: string;
  alertsUrl?: string | null;
};

const SEVERITY_COLOR: Record<MonitorAlertEmailProps["severity"], string> = {
  info: "#2563eb",
  warning: "#b45309",
  critical: "#dc2626",
};

export function MonitorAlertEmail({
  title,
  message,
  severity,
  code,
  alertsUrl,
}: MonitorAlertEmailProps) {
  const preview = `${severity.toUpperCase()}: ${title}`;

  return (
    <EmailShell preview={preview}>
      <Text
        style={{
          ...pill,
          color: SEVERITY_COLOR[severity],
          borderColor: SEVERITY_COLOR[severity],
        }}
      >
        {severity}
      </Text>
      <EmailHeading>{title}</EmailHeading>
      <EmailText>{message}</EmailText>
      <Section style={details}>
        <Text style={detailLabel}>Alert code</Text>
        <Text style={detailValue}>{code}</Text>
      </Section>
      {alertsUrl ? (
        <Button href={alertsUrl} style={button}>
          Open alerts
        </Button>
      ) : null}
    </EmailShell>
  );
}

const pill = {
  border: "1px solid",
  borderRadius: "999px",
  display: "inline-block",
  fontSize: "12px",
  fontWeight: "700",
  lineHeight: "16px",
  margin: "0 0 14px",
  padding: "4px 10px",
  textTransform: "uppercase" as const,
};

const details = {
  backgroundColor: "#ffffff",
  border: "1px solid #e5e7eb",
  borderRadius: "8px",
  margin: "18px 0",
  padding: "14px",
};

const detailLabel = {
  color: "#6b7280",
  fontSize: "12px",
  lineHeight: "18px",
  margin: "0 0 4px",
};

const detailValue = {
  color: "#111827",
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace',
  fontSize: "13px",
  lineHeight: "20px",
  margin: "0",
};

const button = {
  backgroundColor: "#111827",
  borderRadius: "8px",
  color: "#ffffff",
  display: "inline-block",
  fontSize: "14px",
  fontWeight: "600",
  lineHeight: "20px",
  marginTop: "4px",
  padding: "10px 14px",
  textDecoration: "none",
};
