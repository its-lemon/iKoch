/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./toast.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        medal: {
          bg: "#0B0D12",
          surface: "#131620",
          elevated: "#191D2B",
          border: "#252B3D",
          accent: "#3B82F6",
          purple: "#8B5CF6",
          amber: "#F59E0B",
          emerald: "#10B981",
          rose: "#F43F5E",
        },
      },
      fontFamily: {
        sans: [
          "Inter",
          "Segoe UI",
          "system-ui",
          "-apple-system",
          "BlinkMacSystemFont",
          "sans-serif",
        ],
        mono: ["JetBrains Mono", "Cascadia Code", "Consolas", "monospace"],
      },
    },
  },
  plugins: [],
};
