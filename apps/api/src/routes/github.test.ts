import { describe, expect, test } from "bun:test";
import { createHmac } from "node:crypto";
import { verifyWebhookSignature } from "../integrations/github-utils";

function signatureFor(body: string, secret: string): string {
  return `sha256=${createHmac("sha256", secret).update(body).digest("hex")}`;
}

describe("verifyWebhookSignature", () => {
  test("accepts GitHub sha256 signatures for the raw payload", () => {
    const body = JSON.stringify({ ref: "refs/heads/main" });
    expect(verifyWebhookSignature(body, signatureFor(body, "secret"), "secret")).toBe(true);
  });

  test("rejects changed payloads, wrong secrets, and unsupported prefixes", () => {
    const body = JSON.stringify({ ref: "refs/heads/main" });
    expect(verifyWebhookSignature(`${body}\n`, signatureFor(body, "secret"), "secret")).toBe(false);
    expect(verifyWebhookSignature(body, signatureFor(body, "secret"), "other-secret")).toBe(false);
    expect(verifyWebhookSignature(body, `sha1=${signatureFor(body, "secret")}`, "secret")).toBe(
      false,
    );
  });
});
