import { afterEach, describe, expect, test } from "vitest";
import { followLine, mount, snapshot, unmount } from "./editor";
import { editorForwardsKey } from "./keyboard-interceptor";

describe("editorForwardsKey", () => {
  test("offers only primary-modifier chords to the bindings", () => {
    expect(editorForwardsKey({ metaKey: true, ctrlKey: false }, true)).toBe(true);
    expect(editorForwardsKey({ metaKey: false, ctrlKey: true }, true)).toBe(false);
    expect(editorForwardsKey({ metaKey: false, ctrlKey: true }, false)).toBe(true);
    expect(editorForwardsKey({ metaKey: false, ctrlKey: false }, false)).toBe(false);
  });
});

describe("mount / snapshot", () => {
  afterEach(() => {
    unmount();
    document.body.innerHTML = "";
  });

  test("hands back exactly the text it was given, under its key", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const text = "# 設計\n\n- [ ] a\n\n```sh\nx\n```\n";
    mount(host, text, "7-0", () => {});
    expect(snapshot()).toEqual({ key: "7-0", text });
  });

  test("a second mount replaces the first", () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    mount(host, "one", "1-0", () => {});
    mount(host, "two", "2-0", () => {});
    expect(snapshot()).toEqual({ key: "2-0", text: "two" });
    expect(host.querySelectorAll(".cm-editor")).toHaveLength(1);
  });

  test("nothing is mounted after unmount", () => {
    expect(snapshot()).toBeNull();
  });
});

describe("followLine", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  test("does nothing without a preview to follow", () => {
    expect(() => followLine(3)).not.toThrow();
  });

  test("scrolls the block holding the line into view", () => {
    document.body.innerHTML = `
      <div class="content">
        <article class="markdown-body">
          <h1 data-source-range="1:1-1:7">Title</h1>
          <p data-source-range="3:1-5:4">Para</p>
        </article>
      </div>`;
    const scroller = document.querySelector<HTMLElement>(".content")!;
    const para = document.querySelector<HTMLElement>("p")!;
    const calls: number[] = [];
    scroller.scrollTo = ((options: ScrollToOptions) => {
      calls.push(options.top ?? -1);
    }) as typeof scroller.scrollTo;
    scroller.getBoundingClientRect = () => ({ top: 0, bottom: 300, height: 300 }) as DOMRect;
    para.getBoundingClientRect = () => ({ top: 900, bottom: 950, height: 50 }) as DOMRect;

    followLine(4);
    expect(calls).toEqual([800]);
  });
});
