CREATE TABLE "hetzner_connection" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"api_token" text NOT NULL,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL,
	CONSTRAINT "hetzner_connection_organization_id_unique" UNIQUE("organization_id")
);
--> statement-breakpoint
ALTER TABLE "hetzner_connection" ADD CONSTRAINT "hetzner_connection_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;