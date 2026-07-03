function parsePart(part: string, min: number, max: number): Set<number> | null {
  const values = new Set<number>();
  for (const rawSegment of part.split(",")) {
    const segment = rawSegment.trim();
    if (!segment) return null;
    const [base, stepRaw] = segment.split("/");
    const step = stepRaw ? Number(stepRaw) : 1;
    if (!Number.isInteger(step) || step < 1) return null;

    let start: number;
    let end: number;
    if (base === "*") {
      start = min;
      end = max;
    } else if (base?.includes("-")) {
      const [leftRaw, rightRaw] = base.split("-");
      const left = Number(leftRaw);
      const right = Number(rightRaw);
      if (!Number.isInteger(left) || !Number.isInteger(right)) return null;
      start = left;
      end = right;
    } else {
      const value = Number(base);
      if (!Number.isInteger(value)) return null;
      start = value;
      end = value;
    }

    if (start < min || end > max || start > end) return null;
    for (let value = start; value <= end; value += step) values.add(value);
  }
  return values;
}

function parseCronSchedule(
  schedule: string,
): [Set<number>, Set<number>, Set<number>, Set<number>, Set<number>] | null {
  const parts = schedule.trim().split(/\s+/);
  if (parts.length !== 5) return null;
  const [minute, hour, dayOfMonth, month, dayOfWeek] = [
    parsePart(parts[0]!, 0, 59),
    parsePart(parts[1]!, 0, 23),
    parsePart(parts[2]!, 1, 31),
    parsePart(parts[3]!, 1, 12),
    parsePart(parts[4]!, 0, 7),
  ];
  if (!minute || !hour || !dayOfMonth || !month || !dayOfWeek) return null;
  return [minute, hour, dayOfMonth, month, dayOfWeek];
}

export function cronMatches(schedule: string, date: Date): boolean {
  const parsed = parseCronSchedule(schedule);
  if (!parsed) return false;
  const [minute, hour, dayOfMonth, month, dayOfWeek] = parsed;
  const dow = date.getDay();
  return (
    minute.has(date.getMinutes()) &&
    hour.has(date.getHours()) &&
    dayOfMonth.has(date.getDate()) &&
    month.has(date.getMonth() + 1) &&
    (dayOfWeek.has(dow) || (dow === 0 && dayOfWeek.has(7)))
  );
}

export function validateCronSchedule(schedule: string): string | null {
  if (!schedule.trim()) return "schedule is required";
  return parseCronSchedule(schedule) ? null : "schedule must be a five-field cron expression";
}
