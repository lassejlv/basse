// Symmetric encryption for secrets at rest (e.g. third-party access tokens).
// AES-256-GCM with a key derived from BETTER_AUTH_SECRET. Stored as
// "<iv-base64>:<ciphertext-base64>".

const secret = Bun.env.BETTER_AUTH_SECRET;

if (!secret) {
  throw new Error("BETTER_AUTH_SECRET is required to encrypt secrets");
}

const secretValue = secret;

let keyPromise: Promise<CryptoKey> | null = null;

function getKey(): Promise<CryptoKey> {
  if (!keyPromise) {
    keyPromise = crypto.subtle
      .digest("SHA-256", new TextEncoder().encode(secretValue))
      .then((hash) =>
        crypto.subtle.importKey("raw", hash, { name: "AES-GCM" }, false, ["encrypt", "decrypt"]),
      );
  }

  return keyPromise;
}

function toBase64(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("base64");
}

function fromBase64(value: string): Uint8Array<ArrayBuffer> {
  const buffer = Buffer.from(value, "base64");
  const bytes = new Uint8Array(buffer.byteLength);
  bytes.set(buffer);
  return bytes;
}

export async function encryptSecret(plaintext: string): Promise<string> {
  const key = await getKey();
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ciphertext = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv },
    key,
    new TextEncoder().encode(plaintext),
  );

  return `${toBase64(iv)}:${toBase64(new Uint8Array(ciphertext))}`;
}

export async function decryptSecret(encrypted: string): Promise<string> {
  const [ivPart, dataPart] = encrypted.split(":");

  if (!ivPart || !dataPart) {
    throw new Error("Malformed encrypted value");
  }

  const key = await getKey();
  const plaintext = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv: fromBase64(ivPart) },
    key,
    fromBase64(dataPart),
  );

  return new TextDecoder().decode(plaintext);
}
