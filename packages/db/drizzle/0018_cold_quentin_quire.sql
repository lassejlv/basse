CREATE TABLE "environment_env_var" (
	"id" text PRIMARY KEY NOT NULL,
	"environment_id" text NOT NULL,
	"key" text NOT NULL,
	"value" text NOT NULL,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
CREATE TABLE "project_env_var" (
	"id" text PRIMARY KEY NOT NULL,
	"project_id" text NOT NULL,
	"key" text NOT NULL,
	"value" text NOT NULL,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "environment_env_var" ADD CONSTRAINT "environment_env_var_environment_id_environment_id_fk" FOREIGN KEY ("environment_id") REFERENCES "public"."environment"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "project_env_var" ADD CONSTRAINT "project_env_var_project_id_project_id_fk" FOREIGN KEY ("project_id") REFERENCES "public"."project"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "environment_env_var_environmentId_idx" ON "environment_env_var" USING btree ("environment_id");--> statement-breakpoint
CREATE UNIQUE INDEX "environment_env_var_environmentId_key_uidx" ON "environment_env_var" USING btree ("environment_id","key");--> statement-breakpoint
CREATE INDEX "project_env_var_projectId_idx" ON "project_env_var" USING btree ("project_id");--> statement-breakpoint
CREATE UNIQUE INDEX "project_env_var_projectId_key_uidx" ON "project_env_var" USING btree ("project_id","key");