CREATE TABLE "github_app_installation" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"integration_id" text NOT NULL,
	"installation_id" integer NOT NULL,
	"account_login" text NOT NULL,
	"account_type" text,
	"repository_selection" text,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
CREATE TABLE "github_app_integration" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"app_id" integer NOT NULL,
	"slug" text NOT NULL,
	"name" text NOT NULL,
	"client_id" text,
	"private_key" text NOT NULL,
	"webhook_secret" text,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL,
	CONSTRAINT "github_app_integration_organization_id_unique" UNIQUE("organization_id")
);
--> statement-breakpoint
ALTER TABLE "github_app_installation" ADD CONSTRAINT "github_app_installation_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "github_app_installation" ADD CONSTRAINT "github_app_installation_integration_id_github_app_integration_id_fk" FOREIGN KEY ("integration_id") REFERENCES "public"."github_app_integration"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "github_app_integration" ADD CONSTRAINT "github_app_integration_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "github_app_installation_organizationId_idx" ON "github_app_installation" USING btree ("organization_id");--> statement-breakpoint
CREATE UNIQUE INDEX "github_app_installation_integrationId_installationId_uidx" ON "github_app_installation" USING btree ("integration_id","installation_id");