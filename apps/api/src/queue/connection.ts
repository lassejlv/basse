import IORedis, { type RedisOptions } from "ioredis";

// ioredis connection factory for BullMQ. Every connection MUST carry an 'error'
// listener: ioredis is an EventEmitter, and emitting 'error' with zero listeners
// throws synchronously, which under Bun can take down PID 1 on a transient Redis
// blip. maxRetriesPerRequest:null is mandatory for BullMQ's blocking workers.

const redisUrl = Bun.env.REDIS_URL ?? "redis://127.0.0.1:6379";

export function createConnection(options: RedisOptions = {}): IORedis {
  const connection = new IORedis(redisUrl, {
    maxRetriesPerRequest: null,
    retryStrategy: (attempt) => Math.min(attempt * 200, 5000),
    ...options,
  });

  connection.on("error", (error: Error) => {
    console.error("[redis]", error.message);
  });

  return connection;
}
