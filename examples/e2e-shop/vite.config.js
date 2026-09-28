import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // `npm run dev` sends /api to the real API on port 4174, if one runs
  // (see README.md). The tests answer /api themselves.
  server: {
    proxy: { "/api": { target: "http://localhost:4174", ws: true } },
  },
});
