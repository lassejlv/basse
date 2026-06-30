import {
  Body,
  Container,
  Head,
  Heading,
  Html,
  Preview,
  Section,
  Text,
} from "@react-email/components";
import type { ReactNode } from "react";

export type EmailShellProps = {
  preview: string;
  children: ReactNode;
};

export function EmailShell({ preview, children }: EmailShellProps) {
  return (
    <Html>
      <Head />
      <Preview>{preview}</Preview>
      <Body style={body}>
        <Container style={container}>
          <Section style={brandBar}>
            <Text style={brand}>Basse</Text>
          </Section>
          {children}
          <Text style={footer}>Sent by Basse monitoring.</Text>
        </Container>
      </Body>
    </Html>
  );
}

export function EmailHeading({ children }: { children: ReactNode }) {
  return <Heading style={heading}>{children}</Heading>;
}

export function EmailText({ children }: { children: ReactNode }) {
  return <Text style={text}>{children}</Text>;
}

const body = {
  margin: "0",
  backgroundColor: "#f6f8fb",
  fontFamily:
    '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
};

const container = {
  width: "100%",
  maxWidth: "560px",
  margin: "0 auto",
  padding: "32px 20px",
};

const brandBar = {
  borderBottom: "1px solid #e6e9ef",
  marginBottom: "24px",
  paddingBottom: "16px",
};

const brand = {
  color: "#111827",
  fontSize: "16px",
  fontWeight: "700",
  margin: "0",
};

const heading = {
  color: "#111827",
  fontSize: "24px",
  fontWeight: "700",
  lineHeight: "32px",
  margin: "0 0 12px",
};

const text = {
  color: "#374151",
  fontSize: "14px",
  lineHeight: "22px",
  margin: "0 0 12px",
};

const footer = {
  borderTop: "1px solid #e6e9ef",
  color: "#6b7280",
  fontSize: "12px",
  lineHeight: "18px",
  margin: "24px 0 0",
  paddingTop: "16px",
};
