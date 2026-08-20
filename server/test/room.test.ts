import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";

import { MAX_PARTICIPANTS, RoomError } from "../src/protocol";
import type { Room } from "../src/room";

/** Each test gets its own room code so objects never bleed into each other. */
function room(code: string) {
  return env.ROOM.get(env.ROOM.idFromName(code));
}

describe("Room roster", () => {
  it("registers a participant and reports them", async () => {
    await runInDurableObject(room("roster-basic"), (instance: Room) => {
      const result = instance.join("ana");

      expect(result.self).toMatch(/^[0-9a-f-]{36}$/);
      expect(result.participants).toHaveLength(1);
      expect(result.participants[0]).toMatchObject({
        id: result.self,
        name: "ana",
        muted: false,
        deafened: false,
        sharing: false,
      });
    });
  });

  it("orders the roster oldest-first", async () => {
    await runInDurableObject(room("roster-order"), (instance: Room) => {
      instance.join("first");
      instance.join("second");
      instance.join("third");

      expect(instance.roster().map((p) => p.name)).toEqual([
        "first",
        "second",
        "third",
      ]);
    });
  });

  it("rejects a blank display name", async () => {
    await runInDurableObject(room("roster-name"), (instance: Room) => {
      expect(() => instance.join("   ")).toThrow(RoomError);
      expect(instance.roster()).toHaveLength(0);
    });
  });
});

describe("Room capacity", () => {
  it(`accepts exactly ${MAX_PARTICIPANTS} participants`, async () => {
    await runInDurableObject(room("cap-exact"), (instance: Room) => {
      for (let i = 0; i < MAX_PARTICIPANTS; i += 1) {
        instance.join(`p${i}`);
      }

      expect(instance.roster()).toHaveLength(MAX_PARTICIPANTS);
    });
  });

  it("rejects the ninth with a typed error", async () => {
    await runInDurableObject(room("cap-overflow"), (instance: Room) => {
      for (let i = 0; i < MAX_PARTICIPANTS; i += 1) {
        instance.join(`p${i}`);
      }

      try {
        instance.join("one-too-many");
        expect.unreachable("join should have thrown");
      } catch (error) {
        expect(error).toBeInstanceOf(RoomError);
        expect((error as RoomError).code).toBe("room_full");
        expect((error as RoomError).status).toBe(409);
      }

      expect(instance.roster()).toHaveLength(MAX_PARTICIPANTS);
    });
  });

  it("frees the slot again after someone leaves", async () => {
    await runInDurableObject(room("cap-reuse"), (instance: Room) => {
      const ids = Array.from({ length: MAX_PARTICIPANTS }, (_, i) =>
        instance.join(`p${i}`),
      ).map((r) => r.self);

      instance.leave(ids[0]!);
      expect(() => instance.join("newcomer")).not.toThrow();
      expect(instance.roster()).toHaveLength(MAX_PARTICIPANTS);
    });
  });
});

describe("Room teardown", () => {
  it("removes only the participant who left", async () => {
    await runInDurableObject(room("leave-one"), (instance: Room) => {
      const ana = instance.join("ana").self;
      instance.join("bruno");

      instance.leave(ana);

      expect(instance.roster().map((p) => p.name)).toEqual(["bruno"]);
    });
  });

  it("ignores a leave for an unknown id", async () => {
    await runInDurableObject(room("leave-unknown"), (instance: Room) => {
      instance.join("ana");

      expect(() => instance.leave("not-a-real-id")).not.toThrow();
      expect(instance.roster()).toHaveLength(1);
    });
  });

  it("leaves zero state behind when the last participant goes", async () => {
    await runInDurableObject(room("leave-last"), (instance: Room) => {
      const ana = instance.join("ana").self;
      const bruno = instance.join("bruno").self;

      instance.leave(ana);
      instance.leave(bruno);

      expect(instance.roster()).toEqual([]);

      // A fresh join must look exactly like the first one ever.
      const next = instance.join("carla");
      expect(next.participants).toHaveLength(1);
      expect(next.participants[0]?.name).toBe("carla");
    });
  });
});
