import { toastManager } from "@/components/ui/toast";

type ToastType = "success" | "error" | "info" | "warning" | "loading";

interface ToastOptions {
  description?: string;
  timeout?: number;
}

function show(type: ToastType, title: string, options?: ToastOptions): string {
  return toastManager.add({ title, type, ...options });
}

/** Pull a human-readable message out of whatever a mutation threw. */
export function toMessage(error: unknown, fallback = "Something went wrong."): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return fallback;
}

export const toast = {
  success: (title: string, options?: ToastOptions) => show("success", title, options),
  error: (title: string, options?: ToastOptions) => show("error", title, options),
  info: (title: string, options?: ToastOptions) => show("info", title, options),
  warning: (title: string, options?: ToastOptions) => show("warning", title, options),
  loading: (title: string, options?: ToastOptions) => show("loading", title, options),
  message: (title: string, options?: ToastOptions) => toastManager.add({ title, ...options }),
  dismiss: (id?: string) => toastManager.close(id),
};
