import { migrate } from "drizzle-orm/bun-sql/migrator";
import { db } from "./client";

const migrationsFolder = `${import.meta.dir}/../drizzle`;

export async function runMigrations(): Promise<void> {
  await migrate(db, { migrationsFolder });
}
