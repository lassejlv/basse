CREATE TABLE "domain" (
	"id" text PRIMARY KEY NOT NULL,
	"server_id" text NOT NULL,
	"app_id" text,
	"host" text NOT NULL,
	"upstream" text NOT NULL,
	"status" text DEFAULT 'pending' NOT NULL,
	"status_message" text,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "domain" ADD CONSTRAINT "domain_server_id_server_id_fk" FOREIGN KEY ("server_id") REFERENCES "public"."server"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "domain" ADD CONSTRAINT "domain_app_id_app_id_fk" FOREIGN KEY ("app_id") REFERENCES "public"."app"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "domain_serverId_idx" ON "domain" USING btree ("server_id");--> statement-breakpoint
CREATE UNIQUE INDEX "domain_host_uidx" ON "domain" USING btree ("host");