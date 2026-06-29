import { migrate } from "drizzle-orm/bun-sql/migrator";
import { db } from "./client";

// Applies pending migrations using the same bun:sql connection the app uses,
// so no separate Postgres driver (pg/postgres) is needed. Run on startup.
const migrationsFolder = `${import.meta.dir}/../drizzle`;

await migrate(db, { migrationsFolder });

console.log("Migrations applied");
process.exit(0);
