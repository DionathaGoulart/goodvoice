import {
  cloudflarePool,
  cloudflareTest,
} from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";

import { fakeRealtimeApi, TEST_SFU } from "./test/fake-realtime.ts";

const options = {
  wrangler: { configPath: "./wrangler.toml" },
  miniflare: {
    bindings: {
      CALLS_APP_ID: TEST_SFU.appId,
      CALLS_APP_SECRET: TEST_SFU.appSecret,
      TURN_KEY_ID: TEST_SFU.turnKeyId,
      TURN_KEY_API_TOKEN: TEST_SFU.turnKeyToken,
    },
    // Every outbound request from the Worker lands here, so tests never touch
    // the real Realtime API.
    outboundService: fakeRealtimeApi,
  },
};

export default defineConfig({
  plugins: [cloudflareTest(options)],
  test: {
    pool: cloudflarePool(options),
  },
});
