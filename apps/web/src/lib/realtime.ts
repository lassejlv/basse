import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

// Client half of the realtime layer (see apps/api/src/realtime.ts): a single
// workspace-scoped WebSocket delivering small hint events, mapped here to
// React Query invalidations. Data always flows through the normal fetch layer.

type RealtimeEvent = {
  type: "deployment" | "backup" | "staged-changes" | "alert" | "server" | "domain";
  appId?: string;
  projectId?: string;
  serverId?: string;
};

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";
const PING_INTERVAL_MS = 30_000;
const MAX_RECONNECT_DELAY_MS = 30_000;

/**
 * Keeps a realtime socket open for the active workspace and invalidates the
 * matching queries on every event. Mount once in the authed layout; pass the
 * active workspace id so switching workspaces reconnects with the new scope.
 */
export function useRealtime(workspaceId: string | null | undefined): void {
  const queryClient = useQueryClient();

  useEffect(() => {
    if (!workspaceId) return;

    let socket: WebSocket | null = null;
    let closed = false;
    let attempts = 0;
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;

    // Coalesce bursts: events buffer for a beat and duplicates collapse, so a
    // rapid stream (deploy transitions, provisioning) causes one refetch round
    // instead of one per frame.
    const pending = new Map<string, RealtimeEvent>();
    let flushTimer: ReturnType<typeof setTimeout> | undefined;

    function enqueue(event: RealtimeEvent) {
      pending.set(
        `${event.type}:${event.appId ?? ""}:${event.serverId ?? ""}:${event.projectId ?? ""}`,
        event,
      );
      flushTimer ??= setTimeout(() => {
        flushTimer = undefined;
        const events = [...pending.values()];
        pending.clear();
        for (const item of events) invalidate(item);
      }, 300);
    }

    function invalidate(event: RealtimeEvent) {
      switch (event.type) {
        case "deployment":
          void queryClient.invalidateQueries({ queryKey: ["deployments", event.appId] });
          void queryClient.invalidateQueries({ queryKey: ["app", event.appId] });
          void queryClient.invalidateQueries({ queryKey: ["apps"] });
          break;
        case "backup":
          void queryClient.invalidateQueries({ queryKey: ["backups", event.appId] });
          break;
        case "staged-changes":
          if (event.appId) {
            void queryClient.invalidateQueries({ queryKey: ["changes", event.appId] });
            void queryClient.invalidateQueries({ queryKey: ["change-history", event.appId] });
          } else {
            void queryClient.invalidateQueries({ queryKey: ["changes"] });
            void queryClient.invalidateQueries({ queryKey: ["change-history"] });
          }
          void queryClient.invalidateQueries({ queryKey: ["project-changes"] });
          void queryClient.invalidateQueries({ queryKey: ["project-change-history"] });
          break;
        case "alert":
          void queryClient.invalidateQueries({ queryKey: ["alerts"] });
          void queryClient.invalidateQueries({ queryKey: ["alerts-overview"] });
          break;
        case "server":
          void queryClient.invalidateQueries({ queryKey: ["servers"] });
          void queryClient.invalidateQueries({ queryKey: ["server", event.serverId] });
          break;
        case "domain":
          void queryClient.invalidateQueries({ queryKey: ["domains"] });
          break;
      }
    }

    function connect() {
      const url = new URL("/api/ws", apiBaseUrl || window.location.origin);
      url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
      socket = new WebSocket(url);
      socket.onopen = () => {
        attempts = 0;
      };
      socket.onmessage = (event) => {
        if (event.data === "pong") return;
        try {
          enqueue(JSON.parse(event.data as string) as RealtimeEvent);
        } catch {
          // Ignore malformed frames.
        }
      };
      socket.onclose = () => {
        if (closed) return;
        const delay = Math.min(MAX_RECONNECT_DELAY_MS, 1000 * 2 ** attempts);
        attempts += 1;
        reconnectTimer = setTimeout(connect, delay);
      };
    }

    const pingTimer = setInterval(() => {
      if (socket?.readyState === WebSocket.OPEN) socket.send("ping");
    }, PING_INTERVAL_MS);

    connect();

    return () => {
      closed = true;
      clearTimeout(reconnectTimer);
      clearTimeout(flushTimer);
      clearInterval(pingTimer);
      socket?.close();
    };
  }, [queryClient, workspaceId]);
}
