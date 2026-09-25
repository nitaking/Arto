import { afterEach, describe, expect, test } from "vitest";
import {
  continueList,
  followLine,
  indentBlock,
  insideLiteralBlock,
  lineAt,
  offsetOfLine,
  outdentBlock,
} from "./editor";
import { editorForwardsKey } from "./keyboard-interceptor";

describe("lineAt / offsetOfLine", () => {
  const text = "one\ntwo\n\nfour";

  test("name the same line from both ends", () => {
    for (let line = 1; line <= 4; line++) {
      expect(lineAt(text, offsetOfLine(text, line))).toBe(line);
    }
  });

  test("count the caret after a newline as the next line", () => {
    expect(lineAt(text, 0)).toBe(1);
    expect(lineAt(text, 3)).toBe(1);
    expect(lineAt(text, 4)).toBe(2);
  });

  test("clamp to the text", () => {
    expect(offsetOfLine(text, 0)).toBe(0);
    expect(offsetOfLine(text, 99)).toBe(text.length);
    expect(lineAt(text, 999)).toBe(4);
  });
});

describe("continueList", () => {
  test("repeats a bullet with its indentation", () => {
    expect(continueList("  - item")).toEqual({ insert: "\n  - " });
    expect(continueList("* item")).toEqual({ insert: "\n* " });
  });

  test("counts an ordered list on", () => {
    expect(continueList("9. ninth")).toEqual({ insert: "\n10. " });
    expect(continueList("1) first")).toEqual({ insert: "\n2) " });
  });

  test("starts the next task unchecked", () => {
    expect(continueList("- [x] done")).toEqual({ insert: "\n- [ ] " });
  });

  test("ends the list on an empty item", () => {
    expect(continueList("- ")).toEqual({ end: true });
    expect(continueList("3. ")).toEqual({ end: true });
    expect(continueList("- [ ] ")).toEqual({ end: true });
  });

  test("leaves anything else alone", () => {
    expect(continueList("plain text")).toBeNull();
    expect(continueList("-not a list")).toBeNull();
    expect(continueList("")).toBeNull();
  });
});

describe("insideLiteralBlock", () => {
  const at = (text: string) => insideLiteralBlock(text.replace("|", ""), text.indexOf("|"));

  test("is false in ordinary prose", () => {
    expect(at("- a\n- b|")).toBe(false);
  });

  test("is true inside a fence and false after it closes", () => {
    expect(at("```\n- a|")).toBe(true);
    expect(at("```sh\n- a\n```\n- b|")).toBe(false);
    expect(at("~~~\n```\n- a|")).toBe(true);
  });

  test("is true inside front matter", () => {
    expect(at("---\ntags:\n- a|")).toBe(true);
    expect(at("---\ntitle: x\n---\n- a|")).toBe(false);
  });
});

describe("indentBlock / outdentBlock", () => {
  test("indent every non-empty line by one step", () => {
    expect(indentBlock("a\n\nb")).toBe("  a\n\n  b");
  });

  test("outdent removes at most one step", () => {
    expect(outdentBlock("    a\n  b\nc\n\td")).toBe("  a\nb\nc\nd");
  });

  test("outdent undoes indent", () => {
    const block = "- a\n  - b\n\ntext";
    expect(outdentBlock(indentBlock(block))).toBe(block);
  });
});

describe("editorForwardsKey", () => {
  test("offers only primary-modifier chords to the bindings", () => {
    expect(editorForwardsKey({ metaKey: true, ctrlKey: false }, true)).toBe(true);
    expect(editorForwardsKey({ metaKey: false, ctrlKey: true }, true)).toBe(false);
    expect(editorForwardsKey({ metaKey: false, ctrlKey: true }, false)).toBe(true);
    expect(editorForwardsKey({ metaKey: false, ctrlKey: false }, false)).toBe(false);
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
