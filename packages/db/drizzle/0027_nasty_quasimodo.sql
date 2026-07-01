CREATE TABLE "agent_command" (
	"id" text PRIMARY KEY NOT NULL,
	"server_id" text NOT NULL,
	"method" text NOT NULL,
	"path" text NOT NULL,
	"body" text,
	"status" text DEFAULT 'queued' NOT NULL,
	"response_status" integer,
	"response_body" text,
	"error" text,
	"lease_until" timestamp,
	"completed_at" timestamp,
	"created_at" timestamp NOT NULL,
	"updated_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "agent_token_hash" text;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "connection_mode" text DEFAULT 'ssh' NOT NULL;--> statement-breakpoint
ALTER TABLE "agent_command" ADD CONSTRAINT "agent_command_server_id_server_id_fk" FOREIGN KEY ("server_id") REFERENCES "public"."server"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "agent_command_serverId_status_idx" ON "agent_command" USING btree ("server_id","status");--> statement-breakpoint
CREATE INDEX "agent_command_createdAt_idx" ON "agent_command" USING btree ("created_at");