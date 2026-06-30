CREATE TABLE "staged_change" (
	"id" text PRIMARY KEY NOT NULL,
	"app_id" text NOT NULL,
	"resource" text NOT NULL,
	"action" text NOT NULL,
	"field" text NOT NULL,
	"value" text,
	"previous_value" text,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "staged_change" ADD CONSTRAINT "staged_change_app_id_app_id_fk" FOREIGN KEY ("app_id") REFERENCES "public"."app"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "staged_change_appId_idx" ON "staged_change" USING btree ("app_id");--> statement-breakpoint
CREATE UNIQUE INDEX "staged_change_appId_resource_field_uidx" ON "staged_change" USING btree ("app_id","resource","field");