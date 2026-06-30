CREATE TABLE "app_server" (
	"app_id" text NOT NULL,
	"server_id" text NOT NULL,
	"created_at" timestamp NOT NULL
);
--> statement-breakpoint
INSERT INTO "app_server" ("app_id", "server_id", "created_at")
SELECT "id", "server_id", COALESCE("updated_at", "created_at", now())
FROM "app"
WHERE "server_id" IS NOT NULL;
--> statement-breakpoint
ALTER TABLE "app_server" ADD CONSTRAINT "app_server_app_id_app_id_fk" FOREIGN KEY ("app_id") REFERENCES "public"."app"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "app_server" ADD CONSTRAINT "app_server_server_id_server_id_fk" FOREIGN KEY ("server_id") REFERENCES "public"."server"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE UNIQUE INDEX "app_server_appId_serverId_uidx" ON "app_server" USING btree ("app_id","server_id");--> statement-breakpoint
CREATE INDEX "app_server_appId_idx" ON "app_server" USING btree ("app_id");--> statement-breakpoint
CREATE INDEX "app_server_serverId_idx" ON "app_server" USING btree ("server_id");
