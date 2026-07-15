import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

export default defineConfig({
  base: "/dashboard/",
  plugins: [svelte()],
  server: {
    port: 5173,
    proxy: {
      "/v1": "http://127.0.0.1:8080"
    }
  }
});
