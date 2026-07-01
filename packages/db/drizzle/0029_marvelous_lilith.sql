CREATE TABLE "s3_connection" (
	"id" text PRIMARY KEY NOT NULL,
	"organization_id" text NOT NULL,
	"name" text NOT NULL,
	"endpoint" text,
	"region" text,
	"bucket" text NOT NULL,
	"access_key_id" text NOT NULL,
	"secret_access_key" text NOT NULL,
	"secret_hint" text,
	"status" text DEFAULT 'active' NOT NULL,
	"status_message" text,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "backup_s3_connection_id" text;--> statement-breakpoint
ALTER TABLE "database_backup" ADD COLUMN "s3_connection_id" text;--> statement-breakpoint
ALTER TABLE "database_backup" ADD COLUMN "s3_status" text;--> statement-breakpoint
ALTER TABLE "database_backup" ADD COLUMN "s3_key" text;--> statement-breakpoint
ALTER TABLE "database_backup" ADD COLUMN "s3_error" text;--> statement-breakpoint
ALTER TABLE "database_backup" ADD COLUMN "s3_uploaded_at" timestamp;--> statement-breakpoint
ALTER TABLE "s3_connection" ADD CONSTRAINT "s3_connection_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "s3_connection_organizationId_idx" ON "s3_connection" USING btree ("organization_id");--> statement-breakpoint
ALTER TABLE "database_backup" ADD CONSTRAINT "database_backup_s3_connection_id_s3_connection_id_fk" FOREIGN KEY ("s3_connection_id") REFERENCES "public"."s3_connection"("id") ON DELETE set null ON UPDATE no action;