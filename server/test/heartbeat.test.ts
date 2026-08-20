import {
  env,
  runDurableObjectAlarm,
  runInDurableObject,
} from "cloudflare:test";
import { describe, expect, it } from "vitest";

import { HEARTBEAT_TIMEOUT_MS, type Room } from "../src/room";

function room(code: string) {
  return env.ROOM.get(env.ROOM.idFromName(code));
}

/** A moment far enough past `from` that every participant looks stale. */
function afterTimeout(from = Date.now()): number {
  return from + HEARTBEAT_TIMEOUT_MS + 1_000;
}

describe("heartbeat sweep", () => {
  it("keeps everyone while they are inside the window", async () => {
    await runInDurableObject(room("sweep-fresh"), (instance: Room) => {
      const ana = instance.join("ana").self;
      const bruno = instance.join("bruno").self;

      // Pin both to the same instant so the boundary is exact rather than
      // whatever the runtime's clock did between the two joins.
      const seen = Date.now();
      instance.touch(ana, seen);
      instance.touch(bruno, seen);

      expect(instance.sweep(seen + HEARTBEAT_TIMEOUT_MS)).toBe(0);
      expect(instance.roster()).toHaveLength(2);

      expect(instance.sweep(seen + HEARTBEAT_TIMEOUT_MS + 1)).toBe(2);
      expect(instance.roster()).toEqual([]);
    });
  });

  it("evicts a participant who went silent", async () => {
    await runInDurableObject(room("sweep-silent"), (instance: Room) => {
      instance.join("ana");

      expect(instance.sweep(afterTimeout())).toBe(1);
      expect(instance.roster()).toEqual([]);
    });
  });

  it("evicts a participant who joined but never opened a socket", async () => {
    await runInDurableObject(room("sweep-nosocket"), (instance: Room) => {
      instance.join("ghost");
      const live = instance.join("ana").self;

      // Only `ana` is heard from; the ghost ages out.
      const now = afterTimeout();
      instance.touch(live, now);

      expect(instance.sweep(now)).toBe(1);
      expect(instance.roster().map((p) => p.name)).toEqual(["ana"]);
    });
  });

  it("frees the capacity an evicted participant held", async () => {
    await runInDurableObject(room("sweep-capacity"), (instance: Room) => {
      for (let i = 0; i < 8; i += 1) {
        instance.join(`p${i}`);
      }
      expect(() => instance.join("blocked")).toThrow();

      instance.sweep(afterTimeout());

      expect(instance.roster()).toEqual([]);
      expect(() => instance.join("newcomer")).not.toThrow();
    });
  });

  it("leaves zero state behind when the sweep empties the room", async () => {
    const stub = room("sweep-empties");

    await runInDurableObject(stub, (instance: Room) => {
      instance.join("ana");
      instance.join("bruno");
      instance.sweep(afterTimeout());

      expect(instance.roster()).toEqual([]);
    });

    await runInDurableObject(stub, async (_instance: Room, state) => {
      await cleared(state);
    });
  });
});

describe("heartbeat alarm", () => {
  it("is scheduled as soon as the room has a member", async () => {
    const stub = room("alarm-scheduled");

    await runInDurableObject(stub, async (instance: Room, state) => {
      instance.join("ana");
      // `join` schedules without awaiting so the response is not delayed.
      await scheduled(state);

      expect(await state.storage.getAlarm()).not.toBeNull();
    });
  });

  it("reschedules itself while the room is still populated", async () => {
    const stub = room("alarm-reschedule");

    await runInDurableObject(stub, async (instance: Room, state) => {
      instance.join("ana");
      await scheduled(state);
    });

    expect(await runDurableObjectAlarm(stub)).toBe(true);

    await runInDurableObject(stub, async (instance: Room, state) => {
      expect(instance.roster()).toHaveLength(1);
      expect(await state.storage.getAlarm()).not.toBeNull();
    });
  });

  it("stops rescheduling once the room is empty", async () => {
    const stub = room("alarm-stops");

    await runInDurableObject(stub, async (instance: Room, state) => {
      const ana = instance.join("ana").self;
      await scheduled(state);
      instance.leave(ana);
    });

    await runInDurableObject(stub, async (_instance: Room, state) => {
      await cleared(state);
    });
  });
});

/**
 * Alarm bookkeeping is fire-and-forget so it never delays a join or a leave;
 * these two wait for it to converge.
 */
async function scheduled(state: DurableObjectState): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if ((await state.storage.getAlarm()) !== null) {
      return;
    }
  }
  throw new Error("alarm was never scheduled");
}

async function cleared(state: DurableObjectState): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if ((await state.storage.getAlarm()) === null) {
      return;
    }
  }
  throw new Error("alarm was never cleared");
}
