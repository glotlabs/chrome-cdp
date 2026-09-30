import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { devBuild } from "../shared/build.mjs";

export default defineConfig(devBuild("vue", [vue()]));
