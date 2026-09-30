import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { devBuild } from "../shared/build.mjs";

export default defineConfig(devBuild("svelte", [svelte({ compilerOptions: { dev: true } })]));
