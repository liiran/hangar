import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// `npm run dev` serves the UI on :5173 and proxies the API to a running
// daemon. To sign in, run `hangar up --no-open` and open the printed URL with
// the host changed to localhost:5173 (keep the `#pair=...` part).
const daemon = process.env.HANGAR_UI ?? "http://127.0.0.1:7317";
const devPort = 5173;
const devOrigins = new Set([`http://localhost:${devPort}`, `http://127.0.0.1:${devPort}`]);

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: { outDir: "dist", emptyOutDir: true, chunkSizeWarningLimit: 900 },
  server: {
    port: devPort,
    strictPort: true,
    proxy: {
      "/api": {
        target: daemon,
        changeOrigin: true,
        ws: true,
        // The daemon refuses cross-origin requests. Present the dev server's
        // own pages as the daemon's origin, and nothing else: a request from
        // any other site keeps its origin and is refused as usual.
        configure: (proxy) => {
          const rewrite = (req: { getHeader(n: string): unknown; setHeader(n: string, v: string): void }) => {
            const origin = req.getHeader("origin");
            if (typeof origin === "string" && devOrigins.has(origin)) req.setHeader("origin", daemon);
          };
          proxy.on("proxyReq", rewrite);
          proxy.on("proxyReqWs", rewrite);
        },
      },
    },
  },
});
