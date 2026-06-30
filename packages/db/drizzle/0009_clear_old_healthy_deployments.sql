WITH ranked AS (
	SELECT
		"id",
		row_number() OVER (
			PARTITION BY "app_id"
			ORDER BY "created_at" DESC, "id" DESC
		) AS "rank"
	FROM "deployment"
	WHERE "status" = 'healthy'
)
UPDATE "deployment"
SET "status" = 'superseded', "updated_at" = now()
FROM ranked
WHERE "deployment"."id" = ranked."id" AND ranked."rank" > 1;
