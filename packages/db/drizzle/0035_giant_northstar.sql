CREATE TABLE "load_balancer_event" (
	"id" text PRIMARY KEY NOT NULL,
	"load_balancer_id" text NOT NULL,
	"status" text NOT NULL,
	"message" text NOT NULL,
	"details" text,
	"created_at" timestamp NOT NULL
);
--> statement-breakpoint
ALTER TABLE "load_balancer_event" ADD CONSTRAINT "load_balancer_event_load_balancer_id_load_balancer_id_fk" FOREIGN KEY ("load_balancer_id") REFERENCES "public"."load_balancer"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "load_balancer_event_loadBalancerId_idx" ON "load_balancer_event" USING btree ("load_balancer_id");--> statement-breakpoint
CREATE INDEX "load_balancer_event_createdAt_idx" ON "load_balancer_event" USING btree ("created_at");