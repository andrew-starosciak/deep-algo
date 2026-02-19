import type { Config } from "tailwindcss";

const config: Config = {
  content: ["./src/**/*.{js,ts,jsx,tsx,mdx}"],
  theme: {
    extend: {
      colors: {
        bg: {
          primary: "#0a0e17",
          card: "#141824",
          hover: "#1c2130",
        },
        border: {
          DEFAULT: "#2a3142",
        },
        text: {
          primary: "#e0e6ed",
          secondary: "#8b95a5",
        },
        profit: "#00c853",
        loss: "#ff1744",
        accent: "#448aff",
      },
    },
  },
  plugins: [],
};

export default config;
