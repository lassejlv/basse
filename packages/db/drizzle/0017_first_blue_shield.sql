CREATE TABLE "staged_change_history" (
	"id" text PRIMARY KEY NOT NULL,
	"batch_id" text NOT NULL,
	"app_id" text NOT NULL,
	"deployment_id" text,
	"outcome" text NOT NULL,
	"resource" text NOT NULL,
	"action" text NOT NULL,
	"field" text NOT NULL,
	"value" text,
	"previous_value" text,
	"staged_at" timestamp NOT NULL,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "staged_change_history" ADD CONSTRAINT "staged_change_history_app_id_app_id_fk" FOREIGN KEY ("app_id") REFERENCES "public"."app"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "staged_change_history" ADD CONSTRAINT "staged_change_history_deployment_id_deployment_id_fk" FOREIGN KEY ("deployment_id") REFERENCES "public"."deployment"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "staged_change_history_appId_idx" ON "staged_change_history" USING btree ("app_id");--> statement-breakpoint
CREATE INDEX "staged_change_history_batchId_idx" ON "staged_change_history" USING btree ("batch_id");--> statement-breakpoint
CREATE INDEX "staged_change_history_deploymentId_idx" ON "staged_change_history" USING btree ("deployment_id");