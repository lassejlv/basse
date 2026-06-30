import type { ImgHTMLAttributes } from "react";
import type { DatabaseKind } from "@basse/shared";
import postgresIcon from "@/assets/database/postgres.svg";
import redisIcon from "@/assets/database/redis.svg";
import { cn } from "@/lib/utils";

const databaseIcons = {
  postgres: postgresIcon,
  redis: redisIcon,
} satisfies Record<DatabaseKind, string>;

export function databaseEngineLabel(kind: DatabaseKind): string {
  return kind === "redis" ? "Redis" : "Postgres";
}

type DatabaseIconProps = Omit<ImgHTMLAttributes<HTMLImageElement>, "alt" | "src"> & {
  kind: DatabaseKind;
};

export function DatabaseIcon({ className, kind, ...props }: DatabaseIconProps) {
  return (
    <img
      alt=""
      aria-hidden="true"
      className={cn("size-5 shrink-0 object-contain", className)}
      src={databaseIcons[kind]}
      {...props}
    />
  );
}
