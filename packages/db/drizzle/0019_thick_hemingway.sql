CREATE TABLE "alert" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"severity" text NOT NULL,
	"status" text DEFAULT 'open' NOT NULL,
	"code" text NOT NULL,
	"title" text NOT NULL,
	"message" text NOT NULL,
	"fingerprint" text NOT NULL,
	"server_id" text,
	"app_id" text,
	"deployment_id" text,
	"first_seen_at" timestamp NOT NULL,
	"last_seen_at" timestamp NOT NULL,
	"acknowledged_at" timestamp,
	"resolved_at" timestamp,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
CREATE TABLE "monitor_event" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"severity" text NOT NULL,
	"code" text NOT NULL,
	"title" text NOT NULL,
	"message" text NOT NULL,
	"fingerprint" text NOT NULL,
	"server_id" text,
	"app_id" text,
	"deployment_id" text,
	"created_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "alert" ADD CONSTRAINT "alert_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "alert" ADD CONSTRAINT "alert_server_id_server_id_fk" FOREIGN KEY ("server_id") REFERENCES "public"."server"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "alert" ADD CONSTRAINT "alert_app_id_app_id_fk" FOREIGN KEY ("app_id") REFERENCES "public"."app"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "alert" ADD CONSTRAINT "alert_deployment_id_deployment_id_fk" FOREIGN KEY ("deployment_id") REFERENCES "public"."deployment"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "monitor_event" ADD CONSTRAINT "monitor_event_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "monitor_event" ADD CONSTRAINT "monitor_event_server_id_server_id_fk" FOREIGN KEY ("server_id") REFERENCES "public"."server"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "monitor_event" ADD CONSTRAINT "monitor_event_app_id_app_id_fk" FOREIGN KEY ("app_id") REFERENCES "public"."app"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "monitor_event" ADD CONSTRAINT "monitor_event_deployment_id_deployment_id_fk" FOREIGN KEY ("deployment_id") REFERENCES "public"."deployment"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "alert_organizationId_idx" ON "alert" USING btree ("organization_id");--> statement-breakpoint
CREATE INDEX "alert_status_idx" ON "alert" USING btree ("status");--> statement-breakpoint
CREATE INDEX "alert_fingerprint_idx" ON "alert" USING btree ("fingerprint");--> statement-breakpoint
CREATE INDEX "alert_serverId_idx" ON "alert" USING btree ("server_id");--> statement-breakpoint
CREATE INDEX "alert_appId_idx" ON "alert" USING btree ("app_id");--> statement-breakpoint
CREATE INDEX "monitor_event_organizationId_idx" ON "monitor_event" USING btree ("organization_id");--> statement-breakpoint
CREATE INDEX "monitor_event_fingerprint_idx" ON "monitor_event" USING btree ("fingerprint");--> statement-breakpoint
CREATE INDEX "monitor_event_createdAt_idx" ON "monitor_event" USING btree ("created_at");--> statement-breakpoint
CREATE INDEX "monitor_event_serverId_idx" ON "monitor_event" USING btree ("server_id");--> statement-breakpoint
CREATE INDEX "monitor_event_appId_idx" ON "monitor_event" USING btree ("app_id");