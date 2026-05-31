/// <reference types="node" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

const DAEMON_API = process.env.DAEMON_API ?? "http://127.0.0.1:17802";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": path.resolve(__dirname, "./src") },
  },
  server: {
    port: 5273,
    proxy: {
      "/api": {
        target: DAEMON_API,
        changeOrigin: true,
        configure: (proxy: any) => {
          // Rewrite the browser dev-server Origin to the daemon's own origin so
          // the daemon's same-origin guard admits it via the legitimate
          // matching-port path (not the Origin-less bypass). changeOrigin already
          // rewrites Host to the target, so Origin and Host now agree on port.
          // Dev-only: production serves the SPA same-origin with no proxy.
          proxy.on("proxyReq", (proxyReq: any) => proxyReq.setHeader("origin", DAEMON_API));
        },
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
