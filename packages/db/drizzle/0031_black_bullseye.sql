ALTER TABLE "app" ADD COLUMN "health_check_enabled" boolean DEFAULT false NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "health_check_path" text DEFAULT '/' NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "health_check_status" integer DEFAULT 200 NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "health_check_timeout_seconds" integer DEFAULT 5 NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "health_check_interval_seconds" integer DEFAULT 30 NOT NULL;