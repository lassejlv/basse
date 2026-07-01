import { runMigrations } from "./migrations";

await runMigrations();
console.log("Migrations applied");
process.exit(0);
