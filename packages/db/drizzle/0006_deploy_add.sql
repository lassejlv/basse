CREATE TABLE "env_var" (
	"id" text PRIMARY KEY NOT NULL,
	"app_id" text NOT NULL,
	"key" text NOT NULL,
	"value" text NOT NULL,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
CREATE TABLE "environment" (
	"id" text PRIMARY KEY NOT NULL,
	"project_id" text NOT NULL,
	"name" text NOT NULL,
	"slug" text NOT NULL,
	"is_default" boolean DEFAULT false NOT NULL,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "app" DROP CONSTRAINT "app_slug_unique";--> statement-breakpoint
ALTER TABLE "app" ALTER COLUMN "project_id" DROP NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ALTER COLUMN "build_mode" SET DEFAULT 'auto';--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "environment_id" text NOT NULL;--> statement-breakpoint
ALTER TABLE "app" ADD COLUMN "port" integer DEFAULT 3000 NOT NULL;--> statement-breakpoint
ALTER TABLE "deployment" ADD COLUMN "build_id" text;--> statement-breakpoint
ALTER TABLE "deployment" ADD COLUMN "logs" text;--> statement-breakpoint
ALTER TABLE "depot_connection" ADD COLUMN "org_id" text;--> statement-breakpoint
ALTER TABLE "env_var" ADD CONSTRAINT "env_var_app_id_app_id_fk" FOREIGN KEY ("app_id") REFERENCES "public"."app"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "environment" ADD CONSTRAINT "environment_project_id_project_id_fk" FOREIGN KEY ("project_id") REFERENCES "public"."project"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "env_var_appId_idx" ON "env_var" USING btree ("app_id");--> statement-breakpoint
CREATE UNIQUE INDEX "env_var_appId_key_uidx" ON "env_var" USING btree ("app_id","key");--> statement-breakpoint
CREATE INDEX "environment_projectId_idx" ON "environment" USING btree ("project_id");--> statement-breakpoint
CREATE UNIQUE INDEX "environment_projectId_slug_uidx" ON "environment" USING btree ("project_id","slug");--> statement-breakpoint
ALTER TABLE "app" ADD CONSTRAINT "app_environment_id_environment_id_fk" FOREIGN KEY ("environment_id") REFERENCES "public"."environment"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "app_environmentId_idx" ON "app" USING btree ("environment_id");--> statement-breakpoint
CREATE UNIQUE INDEX "app_environmentId_slug_uidx" ON "app" USING btree ("environment_id","slug");