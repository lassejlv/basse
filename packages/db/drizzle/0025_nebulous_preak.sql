ALTER TABLE "app" ADD COLUMN "build_root_directory" text DEFAULT '' NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "dockerfile_path" text DEFAULT 'Dockerfile' NOT NULL;