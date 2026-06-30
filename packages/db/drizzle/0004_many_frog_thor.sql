ALTER TABLE "server" ALTER COLUMN "agent_url" DROP NOT NULL;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "organization_id" text NOT NULL;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "ssh_host" text NOT NULL;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "ssh_port" integer DEFAULT 22 NOT NULL;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "ssh_user" text DEFAULT 'root' NOT NULL;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "ssh_public_key" text NOT NULL;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "ssh_private_key" text NOT NULL;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "agent_token" text;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "host_key_fingerprint" text;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "status" text DEFAULT 'pending' NOT NULL;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "status_message" text;--> statement-breakpoint
ALTER TABLE "server" ADD COLUMN "last_seen_at" timestamp;--> statement-breakpoint
ALTER TABLE "server" ADD CONSTRAINT "server_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "server_organizationId_idx" ON "server" USING btree ("organization_id");