// @ts-check
import { defineConfig } from "astro/config";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  site: "https://snowdamiz.github.io",
  base: "/editur",
  vite: {
    plugins: [tailwindcss()],
  },
});
