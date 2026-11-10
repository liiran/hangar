import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// `npm run dev` proxies to a running daemon (`hangar up --no-open` prints the
// auth URL; open it once on the daemon's port to get the cookie).
const daemon = process.env.HANGAR_UI ?? "http://127.0.0.1:7317";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: { outDir: "dist", emptyOutDir: true, chunkSizeWarningLimit: 900 },
  server: {
    proxy: {
      "/api": { target: daemon, changeOrigin: true, ws: true, headers: { origin: daemon } },
      "/auth": { target: daemon, changeOrigin: true },
    },
  },
});
