import {
  cloudflarePool,
  cloudflareTest,
} from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";

const options = { wrangler: { configPath: "./wrangler.toml" } };

export default defineConfig({
  plugins: [cloudflareTest(options)],
  test: {
    pool: cloudflarePool(options),
  },
});
