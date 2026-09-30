import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { devBuild } from "../shared/build.mjs";

export default defineConfig(devBuild("react19", [react()]));
