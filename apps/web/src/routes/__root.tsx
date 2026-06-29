import { Outlet, createRootRoute } from "@tanstack/react-router";

export const Route = createRootRoute({
  component: RootLayout,
});

function RootLayout() {
  return (
    <main className="min-h-svh bg-background text-foreground">
      <Outlet />
    </main>
  );
}
