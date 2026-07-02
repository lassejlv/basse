ALTER TABLE "app" ADD COLUMN "deploy_webhook_url" text;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "deploy_notify_success" boolean DEFAULT false NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "deploy_notify_failure" boolean DEFAULT false NOT NULL;