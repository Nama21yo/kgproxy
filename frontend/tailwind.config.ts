import type { Config } from "tailwindcss";

export default {
  content: ["./index.html", "./src/**/*.{svelte,ts}"],
  theme: {
    extend: {
      colors: {
        ink: "#18202a",
        muted: "#667085",
        line: "#d9dee7",
        brand: "#176b87",
        "brand-strong": "#0b4f66"
      }
    }
  },
  plugins: []
} satisfies Config;
