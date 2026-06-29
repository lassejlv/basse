ALTER TABLE "project" DROP CONSTRAINT "project_slug_unique";--> statement-breakpoint
ALTER TABLE "project" ADD COLUMN "organization_id" text NOT NULL;--> statement-breakpoint
ALTER TABLE "project" ADD CONSTRAINT "project_organization_id_organization_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organization"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "project_organizationId_idx" ON "project" USING btree ("organization_id");--> statement-breakpoint
CREATE UNIQUE INDEX "project_organizationId_slug_uidx" ON "project" USING btree ("organization_id","slug");