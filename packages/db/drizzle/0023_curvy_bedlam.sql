CREATE TABLE "github_webhook_delivery" (
	"id" text PRIMARY KEY NOT NULL,
	"integration_id" text NOT NULL,
	"delivery_id" text NOT NULL,
	"event" text NOT NULL,
	"created_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "github_webhook_delivery" ADD CONSTRAINT "github_webhook_delivery_integration_id_github_app_integration_id_fk" FOREIGN KEY ("integration_id") REFERENCES "public"."github_app_integration"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "github_webhook_delivery_integrationId_idx" ON "github_webhook_delivery" USING btree ("integration_id");--> statement-breakpoint
CREATE UNIQUE INDEX "github_webhook_delivery_integrationId_deliveryId_uidx" ON "github_webhook_delivery" USING btree ("integration_id","delivery_id");