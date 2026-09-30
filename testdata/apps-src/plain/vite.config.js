import { defineConfig } from "vite";
import { devBuild } from "../shared/build.mjs";

export default defineConfig(devBuild("plain", []));
