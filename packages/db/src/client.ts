import { drizzle } from "drizzle-orm/bun-sql";
import * as schema from "./schema";

const databaseUrl = Bun.env.DATABASE_URL ?? "postgres://postgres:postgres@127.0.0.1:5432/basse";

export const db = drizzle({
  connection: {
    url: databaseUrl,
    prepare: false,
    tls: databaseUrl.includes("sslmode=require") ? true : undefined,
  },
  schema,
});
