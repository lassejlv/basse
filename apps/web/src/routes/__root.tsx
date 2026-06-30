import { Outlet, createRootRoute } from "@tanstack/react-router";
import { ToastProvider } from "@/components/ui/toast";

export const Route = createRootRoute({
  component: RootLayout,
});

function RootLayout() {
  return (
    <ToastProvider>
      <main className="min-h-svh bg-background text-foreground">
        <Outlet />
      </main>
    </ToastProvider>
  );
}
