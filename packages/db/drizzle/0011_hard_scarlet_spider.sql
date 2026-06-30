ALTER TABLE "app" ADD COLUMN "source_type" text DEFAULT 'repository' NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "image_ref" text;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "volumes" text DEFAULT '[]' NOT NULL;