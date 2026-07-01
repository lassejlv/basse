import { useQuery } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { useEffect } from "react";
import { getApp } from "@/lib/apps";

// The app experience lives in the project canvas sidecard. This route only
// exists so old /apps/:id links (and API-side references) keep working — it
// resolves the app's project and forwards there with the panel open.
export const Route = createFileRoute("/_authed/apps/$appId")({
  component: AppRedirectRoute,
});

function AppRedirectRoute() {
  const { appId } = Route.useParams();
  const navigate = useNavigate();
  const app = useQuery({ queryKey: ["app", appId], queryFn: () => getApp(appId) });
  const projectId = app.data?.projectId;

  useEffect(() => {
    if (!projectId) return;
    void navigate({
      to: "/projects/$projectId",
      params: { projectId },
      search: { app: appId, tab: undefined },
      replace: true,
    });
  }, [appId, navigate, projectId]);

  if (app.isError) {
    return (
      <p className="p-4 text-destructive-foreground text-sm md:p-6">
        App not found.{" "}
        <Link className="underline underline-offset-4" to="/projects">
          Back to projects
        </Link>
      </p>
    );
  }
  return <p className="p-4 text-muted-foreground text-sm md:p-6">Opening app…</p>;
}
