import { ensureSyntaxTree } from "@codemirror/language";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { EditorSelection, EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import { frontMatterEnd, previewSpans, type PreviewSpan } from "./live-preview";

/** A state for `text`, with the caret where `|` is (or at the end). */
function stateOf(marked: string): EditorState {
  const caret = marked.indexOf("|");
  const doc = caret === -1 ? marked : marked.replace("|", "");
  const state = EditorState.create({
    doc,
    selection: EditorSelection.cursor(caret === -1 ? doc.length : caret),
    extensions: [markdown({ base: markdownLanguage })],
  });
  ensureSyntaxTree(state, state.doc.length, 5000);
  return state;
}

function hidden(state: EditorState, spans: PreviewSpan[]): string[] {
  return spans
    .filter((s): s is Extract<PreviewSpan, { kind: "hide" }> => s.kind === "hide")
    .map((s) => state.doc.sliceString(s.from, s.to));
}

function lineClasses(spans: PreviewSpan[]): string[] {
  return spans
    .filter((s): s is Extract<PreviewSpan, { kind: "line" }> => s.kind === "line")
    .map((s) => s.className);
}

describe("previewSpans", () => {
  test("hides a heading's marks away from the caret, shows them on its line", () => {
    const away = stateOf("## Title\n\ntext|");
    expect(hidden(away, previewSpans(away))).toEqual(["## "]);
    expect(lineClasses(previewSpans(away))).toContain("cm-md-h2");

    const on = stateOf("## Ti|tle\n\ntext");
    expect(hidden(on, previewSpans(on))).toEqual([]);
    expect(lineClasses(previewSpans(on))).toContain("cm-md-h2");
  });

  test("hides emphasis and inline code marks but keeps their text", () => {
    const state = stateOf("**bold** *em* ~~gone~~ `code`\n\n|");
    expect(hidden(state, previewSpans(state))).toEqual([
      "**",
      "**",
      "*",
      "*",
      "~~",
      "~~",
      "`",
      "`",
    ]);
  });

  test("shows a link's text and hides its target", () => {
    const state = stateOf('[docs](https://example.com "Title")\n\n|');
    expect(hidden(state, previewSpans(state))).toEqual(["[", '](https://example.com "Title")']);
  });

  test("leaves an autolink whole", () => {
    const state = stateOf("<https://example.com>\n\n|");
    expect(hidden(state, previewSpans(state))).toEqual([]);
  });

  test("draws bullets and tasks away from the caret", () => {
    const state = stateOf("- a\n- [x] b\n\n|");
    const kinds = previewSpans(state)
      .filter((s) => s.kind === "bullet" || s.kind === "task")
      .map((s) => (s.kind === "task" ? `task:${s.checked}` : s.kind));
    expect(kinds).toEqual(["bullet", "bullet", "task:true"]);
  });

  test("never hides anything inside a fenced code block", () => {
    const state = stateOf("```md\n# not a heading\n**x**\n```\n\n|");
    expect(hidden(state, previewSpans(state))).toEqual([]);
    expect(lineClasses(previewSpans(state)).filter((c) => c === "cm-md-codeblock")).toHaveLength(4);
  });

  test("treats front matter as front matter", () => {
    const state = stateOf("---\ntitle: x\n---\n\n# Body\n|");
    expect(frontMatterEnd(state)).toBe("---\ntitle: x\n---".length);
    const classes = lineClasses(previewSpans(state));
    expect(classes.filter((c) => c === "cm-md-frontmatter")).toHaveLength(3);
    expect(classes).not.toContain("cm-md-h2");
    expect(classes).not.toContain("cm-md-hr");
  });

  test("a document without front matter has none", () => {
    expect(frontMatterEnd(stateOf("# a\n---\n"))).toBe(0);
  });
});
