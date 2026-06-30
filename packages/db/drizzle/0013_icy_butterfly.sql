ALTER TABLE "app" ADD COLUMN "app_kind" text DEFAULT 'service' NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "database_kind" text;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "database_version" text;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "database_name" text;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "database_user" text;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "database_password" text;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "database_public_enabled" boolean DEFAULT false NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "database_public_port" integer;