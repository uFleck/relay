import { describe, expect, it, vi } from "vitest";
import type { V2WorkspaceSnapshot } from "./ipc";
import { WorkspaceStore } from "./workspaceStore";

const snapshot = (id: string) => ({ id }) as unknown as V2WorkspaceSnapshot;

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("WorkspaceStore", () => {
  it("drops an older snapshot that finishes after a newer one", async () => {
    const first = deferred<V2WorkspaceSnapshot>();
    const second = deferred<V2WorkspaceSnapshot>();
    const load = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const store = new WorkspaceStore(load, () => () => {});

    const oldSnapshot = snapshot("old");
    const newSnapshot = snapshot("new");
    const initial = store.refresh();
    const latest = store.refresh();
    second.resolve(newSnapshot);
    await latest;
    first.resolve(oldSnapshot);
    await initial;

    expect(store.getSnapshot().workspace).toBe(newSnapshot);
    expect(store.getSnapshot().version).toBe(1);
  });

  it("coalesces duplicate change signals into one refresh", async () => {
    let notify!: () => void;
    const load = vi.fn().mockResolvedValue(snapshot("one"));
    const store = new WorkspaceStore(load, (listener) => {
      notify = listener;
      return () => {};
    });

    const unsubscribe = store.subscribe(() => {});
    await Promise.resolve();
    await Promise.resolve();
    notify();
    notify();
    await Promise.resolve();
    await Promise.resolve();

    expect(load).toHaveBeenCalledTimes(2);
    unsubscribe();
  });

  it("keeps the last workspace when a refresh fails", async () => {
    const load = vi
      .fn()
      .mockResolvedValueOnce(snapshot("one"))
      .mockRejectedValueOnce(new Error("offline"));
    const store = new WorkspaceStore(load, () => () => {});

    await store.refresh();
    await store.refresh();

    expect(store.getSnapshot().workspace).toBeDefined();
    expect(store.getSnapshot().error).toContain("offline");
  });
});
