/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["templates/**/*.html", "templates/**/*.md"],
  theme: {
    fontFamily: {
      sans: ["Inter"],
      mono: ["monospace"],
      // 'serif': ['Playfair Display', 'system-ui'],
      // 'serif': ['ETBembo', 'system-ui'],
      // 'title': ['Cinzel', 'system-ui'],
      // 'dropcap': ['"Genzsch Initials"', 'system-ui'],
    },
  },
  plugins: [require("@tailwindcss/typography")],
};
