CREATE TABLE "neon_connection" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"api_key" text NOT NULL,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL,
	CONSTRAINT "neon_connection_organization_id_unique" UNIQUE("organization_id")
);
--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "neon_project_id" text;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "neon_region" text;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "neon_connection_uri" text;--> statement-breakpoint
ALTER TABLE "neon_connection" ADD CONSTRAINT "neon_connection_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;