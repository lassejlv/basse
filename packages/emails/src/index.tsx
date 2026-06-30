import { render } from "@react-email/render";
import { MonitorAlertEmail, type MonitorAlertEmailProps } from "./templates/monitor-alert";

export type RenderedEmail = {
  html: string;
  text: string;
};

export type { MonitorAlertEmailProps };

export async function renderMonitorAlertEmail(
  props: MonitorAlertEmailProps,
): Promise<RenderedEmail> {
  const element = <MonitorAlertEmail {...props} />;
  const [html, text] = await Promise.all([render(element), render(element, { plainText: true })]);
  return { html, text };
}
