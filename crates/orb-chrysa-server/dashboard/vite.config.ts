import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid()],
  server: {
    proxy: {
      "/raft": "http://localhost:5050",
      "/v2": "http://localhost:5050",
      "/api": "http://localhost:5050",
    },
  },
  build: {
    target: "esnext",
    outDir: "dist",
    assetsDir: "assets",
  },
});
