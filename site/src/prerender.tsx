import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router"
import { renderToString } from "react-dom/server"
import { routeTree } from "./routeTree.gen"

/**
 * The landing page as static HTML, written into dist/index.html at build time so the page
 * paints before any script runs. main.tsx then renders the live app over it.
 */
export const render = async () => {
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: ["/"] }) })
  await router.load()
  return renderToString(<RouterProvider router={router} />)
}
