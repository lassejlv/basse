CREATE TABLE "load_balancer" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"integration_id" text NOT NULL,
	"app_id" text NOT NULL,
	"provider" text NOT NULL,
	"name" text NOT NULL,
	"host" text NOT NULL,
	"location" text DEFAULT 'fsn1' NOT NULL,
	"load_balancer_type" text DEFAULT 'lb11' NOT NULL,
	"health_check_path" text DEFAULT '/' NOT NULL,
	"provider_resource_id" text,
	"endpoint_ipv4" text,
	"endpoint_ipv6" text,
	"status" text DEFAULT 'pending' NOT NULL,
	"status_message" text,
	"last_synced_at" timestamp,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
CREATE TABLE "load_balancer_integration" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"provider" text NOT NULL,
	"name" text NOT NULL,
	"token" text NOT NULL,
	"token_hint" text,
	"status" text DEFAULT 'active' NOT NULL,
	"status_message" text,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
CREATE TABLE "load_balancer_target" (
	"id" text PRIMARY KEY NOT NULL,
	"load_balancer_id" text NOT NULL,
	"server_id" text NOT NULL,
	"address" text NOT NULL,
	"provider_target_id" text,
	"status" text DEFAULT 'pending' NOT NULL,
	"status_message" text,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
DROP INDEX "domain_host_uidx";--> statement-breakpoint
ALTER TABLE "load_balancer" ADD CONSTRAINT "load_balancer_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "load_balancer" ADD CONSTRAINT "load_balancer_integration_id_load_balancer_integration_id_fk" FOREIGN KEY ("integration_id") REFERENCES "public"."load_balancer_integration"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "load_balancer" ADD CONSTRAINT "load_balancer_app_id_app_id_fk" FOREIGN KEY ("app_id") REFERENCES "public"."app"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "load_balancer_integration" ADD CONSTRAINT "load_balancer_integration_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "load_balancer_target" ADD CONSTRAINT "load_balancer_target_load_balancer_id_load_balancer_id_fk" FOREIGN KEY ("load_balancer_id") REFERENCES "public"."load_balancer"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "load_balancer_target" ADD CONSTRAINT "load_balancer_target_server_id_server_id_fk" FOREIGN KEY ("server_id") REFERENCES "public"."server"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "load_balancer_organizationId_idx" ON "load_balancer" USING btree ("organization_id");--> statement-breakpoint
CREATE INDEX "load_balancer_integrationId_idx" ON "load_balancer" USING btree ("integration_id");--> statement-breakpoint
CREATE UNIQUE INDEX "load_balancer_appId_uidx" ON "load_balancer" USING btree ("app_id");--> statement-breakpoint
CREATE UNIQUE INDEX "load_balancer_organizationId_host_uidx" ON "load_balancer" USING btree ("organization_id","host");--> statement-breakpoint
CREATE INDEX "load_balancer_integration_organizationId_idx" ON "load_balancer_integration" USING btree ("organization_id");--> statement-breakpoint
CREATE UNIQUE INDEX "load_balancer_integration_organizationId_provider_uidx" ON "load_balancer_integration" USING btree ("organization_id","provider");--> statement-breakpoint
CREATE INDEX "load_balancer_target_loadBalancerId_idx" ON "load_balancer_target" USING btree ("load_balancer_id");--> statement-breakpoint
CREATE INDEX "load_balancer_target_serverId_idx" ON "load_balancer_target" USING btree ("server_id");--> statement-breakpoint
CREATE UNIQUE INDEX "load_balancer_target_loadBalancerId_serverId_uidx" ON "load_balancer_target" USING btree ("load_balancer_id","server_id");--> statement-breakpoint
CREATE UNIQUE INDEX "domain_serverId_host_uidx" ON "domain" USING btree ("server_id","host");