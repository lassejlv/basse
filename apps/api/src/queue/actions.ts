import { provisionServer } from "../provision";
import type { ActionName } from "./queue";

// Maps each action to its handler. Imported ONLY by worker.ts — never by the
// enqueue/route path — so provisionServer (and its ssh/agent imports) stay out of
// the HTTP request module graph. Each handler receives the entity id and must
// stay async/non-blocking (synchronous work longer than the worker lock duration
// would stall and re-run the job).
export const actionHandlers: Record<ActionName, (entityId: string) => Promise<void>> = {
  "provision-server": (serverId) => provisionServer(serverId),
};
