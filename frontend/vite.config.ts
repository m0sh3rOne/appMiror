import { defineConfig } from "vite";

// En dev, on proxifie le WebSocket vers le backend Rust (port 8080).
export default defineConfig({
  server: {
    host: true,
    port: 5173,
    proxy: {
      "/ws": {
        target: "ws://127.0.0.1:8080",
        ws: true,
      },
    },
  },
});
