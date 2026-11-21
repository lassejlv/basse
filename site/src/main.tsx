import "./index.css"
import { createRouter, RouterProvider } from "@tanstack/react-router"
import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { routeTree } from "./routeTree.gen"

const router = createRouter({ routeTree })

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router
  }
}

const root = document.getElementById("root")
if (!root) throw new Error("#root is missing from index.html")

// The production build puts a static copy of the page in #root (scripts/prerender.ts) for the
// first paint. The router renders differently on the server, so this replaces that copy
// instead of hydrating it; the markup is the same, so nothing visibly changes.
createRoot(root).render(
  <StrictMode>
    <RouterProvider router={router} />
  </StrictMode>,
)
