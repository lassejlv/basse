CREATE TABLE "app_cron_job" (
	"id" text PRIMARY KEY NOT NULL,
	"app_id" text NOT NULL,
	"name" text NOT NULL,
	"command" text NOT NULL,
	"schedule" text NOT NULL,
	"enabled" boolean DEFAULT true NOT NULL,
	"last_status" text,
	"last_run_at" timestamp,
	"last_finished_at" timestamp,
	"last_output" text,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "app_cron_job" ADD CONSTRAINT "app_cron_job_app_id_app_id_fk" FOREIGN KEY ("app_id") REFERENCES "public"."app"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "app_cron_job_appId_idx" ON "app_cron_job" USING btree ("app_id");--> statement-breakpoint
CREATE INDEX "app_cron_job_enabled_idx" ON "app_cron_job" USING btree ("enabled");