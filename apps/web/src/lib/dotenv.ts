// A pragmatic .env parser/serializer for the Variables raw editor. Round-trips
// the common cases: quoted values, `export` prefixes, comments, and `=` inside
// values. Not a full POSIX shell parser — it does not expand `${VAR}`.
export type EnvPair = { key: string; value: string };

const KEY_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** Parse `.env` text into key/value pairs. Later duplicates win. */
export function parseDotenv(text: string): EnvPair[] {
  const pairs = new Map<string, string>();

  for (const rawLine of text.split("\n")) {
    const line = rawLine.endsWith("\r") ? rawLine.slice(0, -1) : rawLine;
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;

    let body = trimmed;
    if (body.startsWith("export ")) body = body.slice("export ".length).trimStart();

    const eq = body.indexOf("=");
    if (eq === -1) continue;

    const key = body.slice(0, eq).trim();
    if (!KEY_RE.test(key)) continue;

    pairs.set(key, parseValue(body.slice(eq + 1)));
  }

  return [...pairs].map(([key, value]) => ({ key, value }));
}

function parseValue(raw: string): string {
  const value = raw.trim();
  if (value.length === 0) return "";

  const quote = value.charAt(0);
  if (quote === '"') {
    const end = value.lastIndexOf('"');
    return end > 0 ? unescapeDouble(value.slice(1, end)) : value.slice(1);
  }
  if (quote === "'") {
    const end = value.lastIndexOf("'");
    return end > 0 ? value.slice(1, end) : value.slice(1);
  }

  // Unquoted: drop an inline comment (whitespace followed by `#`).
  const comment = value.search(/\s+#/);
  return comment === -1 ? value : value.slice(0, comment).trimEnd();
}

function unescapeDouble(input: string): string {
  let result = "";
  for (let i = 0; i < input.length; i += 1) {
    if (input.charAt(i) === "\\" && i + 1 < input.length) {
      const next = input.charAt(i + 1);
      const mapped =
        next === "n"
          ? "\n"
          : next === "r"
            ? "\r"
            : next === "t"
              ? "\t"
              : next === '"'
                ? '"'
                : next === "\\"
                  ? "\\"
                  : null;
      if (mapped !== null) {
        result += mapped;
        i += 1;
        continue;
      }
    }
    result += input.charAt(i);
  }
  return result;
}

/** Serialize pairs back to `.env` text, quoting values only when needed. */
export function serializeDotenv(vars: EnvPair[]): string {
  return vars.map(({ key, value }) => `${key}=${quoteIfNeeded(value)}`).join("\n");
}

function quoteIfNeeded(value: string): string {
  if (value === "") return "";
  const needsQuote = /[\s#'"]/.test(value) || value !== value.trim();
  if (!needsQuote) return value;

  const escaped = value
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"')
    .replace(/\n/g, "\\n")
    .replace(/\r/g, "\\r")
    .replace(/\t/g, "\\t");
  return `"${escaped}"`;
}
