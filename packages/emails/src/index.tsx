import { render } from "@react-email/render";
import { LoginOtpEmail, type LoginOtpEmailProps, type LoginOtpType } from "./templates/login-otp";
import { MonitorAlertEmail, type MonitorAlertEmailProps } from "./templates/monitor-alert";

export type RenderedEmail = {
  html: string;
  text: string;
};

export type { LoginOtpEmailProps, LoginOtpType, MonitorAlertEmailProps };

export async function renderMonitorAlertEmail(
  props: MonitorAlertEmailProps,
): Promise<RenderedEmail> {
  const element = <MonitorAlertEmail {...props} />;
  const [html, text] = await Promise.all([render(element), render(element, { plainText: true })]);
  return { html, text };
}

export async function renderLoginOtpEmail(props: LoginOtpEmailProps): Promise<RenderedEmail> {
  const element = <LoginOtpEmail {...props} />;
  const [html, text] = await Promise.all([render(element), render(element, { plainText: true })]);
  return { html, text };
}
