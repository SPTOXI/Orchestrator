import { describe, expect, it } from "vitest";
import { createChannel } from "./events";

describe("createChannel", () => {
  it("registers the underlying listener once and fans out", async () => {
    let registrations = 0;
    const channel = createChannel<number>(() => {
      registrations += 1;
      return Promise.resolve();
    });
    const a: number[] = [];
    const b: number[] = [];
    const offA = channel.subscribe((n) => a.push(n));
    channel.subscribe((n) => b.push(n));
    await channel.ready();

    channel.dispatch(1);
    offA();
    channel.dispatch(2);

    expect(registrations).toBe(1);
    expect(a).toEqual([1]);
    expect(b).toEqual([1, 2]);
  });

  it("works without a backend (plain browser)", async () => {
    const channel = createChannel<string>(() => null);
    const seen: string[] = [];
    channel.subscribe((s) => seen.push(s));
    await channel.ready();
    channel.dispatch("x");
    expect(seen).toEqual(["x"]);
  });
});
