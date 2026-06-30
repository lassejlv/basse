import {
  Body,
  Column,
  Container,
  Head,
  Heading,
  Html,
  Preview,
  Row,
  Section,
  Text,
} from "@react-email/components";
import type { CSSProperties, ReactNode } from "react";

export type EmailShellProps = {
  preview: string;
  /** Optional severity/brand color rendered as a thin bar across the top of the card. */
  accent?: string;
  /** Contextual footer note. Defaults to the monitoring opt-in line. */
  footnote?: ReactNode;
  children: ReactNode;
};

export function EmailShell({ preview, accent, footnote, children }: EmailShellProps) {
  return (
    <Html lang="en">
      <Head>
        <meta name="color-scheme" content="dark" />
        <meta name="supported-color-schemes" content="dark" />
      </Head>
      <Preview>{preview}</Preview>
      <Body style={body}>
        <Container style={container}>
          <Section style={brandRow}>
            <Row>
              <Column style={brandMarkCell}>
                <div style={logoMark}>B</div>
              </Column>
              <Column style={brandNameCell}>
                <Text style={wordmark}>Basse</Text>
              </Column>
            </Row>
          </Section>

          <Section style={accent ? { ...card, borderTop: `3px solid ${accent}` } : card}>
            {children}
          </Section>

          <Section style={footerWrap}>
            <Text style={footerText}>
              {footnote ??
                "You’re receiving this because monitoring is enabled for your Basse workspace."}
            </Text>
            <Text style={footerBrand}>Basse &middot; self-hosted deployments</Text>
          </Section>
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

const sans =
  '"Inter", -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif';

const body: CSSProperties = {
  margin: "0",
  padding: "32px 16px",
  backgroundColor: "#0a0a0a",
  fontFamily: sans,
};

const container: CSSProperties = {
  width: "100%",
  maxWidth: "600px",
  margin: "0 auto",
};

const brandRow: CSSProperties = {
  padding: "0 4px 18px",
};

const brandMarkCell: CSSProperties = {
  width: "30px",
  verticalAlign: "middle",
};

const brandNameCell: CSSProperties = {
  verticalAlign: "middle",
  paddingLeft: "10px",
};

const logoMark: CSSProperties = {
  width: "28px",
  height: "28px",
  borderRadius: "7px",
  backgroundColor: "#fafafa",
  color: "#0a0a0a",
  fontSize: "15px",
  fontWeight: "700",
  lineHeight: "28px",
  textAlign: "center",
};

const wordmark: CSSProperties = {
  color: "#fafafa",
  fontSize: "16px",
  fontWeight: "600",
  letterSpacing: "-0.01em",
  margin: "0",
};

const card: CSSProperties = {
  backgroundColor: "#161616",
  border: "1px solid #262626",
  borderRadius: "14px",
  overflow: "hidden",
};

const heading: CSSProperties = {
  color: "#fafafa",
  fontSize: "22px",
  fontWeight: "700",
  lineHeight: "30px",
  letterSpacing: "-0.01em",
  margin: "14px 0 10px",
};

const text: CSSProperties = {
  color: "#a3a3a3",
  fontSize: "15px",
  lineHeight: "24px",
  margin: "0 0 8px",
};

const footerWrap: CSSProperties = {
  padding: "20px 4px 0",
};

const footerText: CSSProperties = {
  color: "#737373",
  fontSize: "12px",
  lineHeight: "18px",
  margin: "0 0 4px",
};

const footerBrand: CSSProperties = {
  color: "#525252",
  fontSize: "12px",
  lineHeight: "18px",
  margin: "0",
};
