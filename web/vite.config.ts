import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  build: {
    // Le serveur Rust sert ce répertoire ; le Dockerfile l'y copie.
    outDir: "dist",
    sourcemap: true,
  },
  server: {
    // En développement, Vite sert le front et relaie l'API au binaire Rust,
    // ce qui évite d'avoir à gérer CORS.
    proxy: {
      "/api": {
        target: process.env.ECOFIN_API ?? "http://127.0.0.1:8099",
        changeOrigin: true,
      },
    },
  },
});
