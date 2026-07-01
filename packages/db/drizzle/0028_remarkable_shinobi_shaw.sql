CREATE TABLE "database_backup" (
	"id" text PRIMARY KEY NOT NULL,
	"app_id" text NOT NULL,
	"server_id" text NOT NULL,
	"status" text DEFAULT 'queued' NOT NULL,
	"trigger" text DEFAULT 'manual' NOT NULL,
	"size_bytes" bigint,
	"error" text,
	"started_at" timestamp,
	"completed_at" timestamp,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "backup_schedule_enabled" boolean DEFAULT false NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "backup_interval_hours" integer DEFAULT 24 NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "backup_retention" integer DEFAULT 7 NOT NULL;--> statement-breakpoint
ALTER TABLE "database_backup" ADD CONSTRAINT "database_backup_app_id_app_id_fk" FOREIGN KEY ("app_id") REFERENCES "public"."app"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "database_backup" ADD CONSTRAINT "database_backup_server_id_server_id_fk" FOREIGN KEY ("server_id") REFERENCES "public"."server"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "database_backup_appId_idx" ON "database_backup" USING btree ("app_id");--> statement-breakpoint
CREATE INDEX "database_backup_appId_status_idx" ON "database_backup" USING btree ("app_id","status");--> statement-breakpoint
CREATE INDEX "database_backup_serverId_idx" ON "database_backup" USING btree ("server_id");