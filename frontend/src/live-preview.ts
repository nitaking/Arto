/**
 * Live preview for the source editor: Markdown drawn as what it means, with
 * the markup shown only where the caret is.
 *
 * The document in the editor is always the Markdown source, byte for byte —
 * this file only decides how each piece of it is *displayed*. A heading
 * line is drawn large with its `#` hidden, `**bold**` is drawn bold with its
 * asterisks hidden, a link shows its text and hides its URL; the line (or
 * lines) the caret is on show everything, so what is being edited is always
 * the real text. Nothing here issues a change to the document except the
 * task checkbox, which toggles `[ ]` and `[x]` exactly as typing would.
 *
 * The parts are split so the decision can be tested without a view:
 * [`previewSpans`] reads an `EditorState` and says what to draw;
 * [`livePreview`] turns that into CodeMirror decorations.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension, Range } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";

/** One thing to draw. Offsets are into the document. */
export type PreviewSpan =
  | { kind: "line"; from: number; className: string }
  | { kind: "mark"; from: number; to: number; className: string }
  | { kind: "hide"; from: number; to: number }
  | { kind: "bullet"; from: number; to: number }
  | { kind: "task"; from: number; to: number; checked: boolean };

/** Where a leading YAML front matter block ends, or 0 when there is none. */
export function frontMatterEnd(state: EditorState): number {
  const doc = state.doc;
  if (doc.lines < 2 || doc.line(1).text !== "---") return 0;
  for (let n = 2; n <= doc.lines; n++) {
    const text = doc.line(n).text;
    if (text === "---" || text === "...") return doc.line(n).to;
  }
  return 0;
}

/** The line numbers any selection range touches: those show their markup. */
function activeLines(state: EditorState): Set<number> {
  const lines = new Set<number>();
  for (const range of state.selection.ranges) {
    const first = state.doc.lineAt(range.from).number;
    const last = state.doc.lineAt(range.to).number;
    for (let n = first; n <= last; n++) lines.add(n);
  }
  return lines;
}

/** Whether the caret is on any line of `[from, to]`. */
function touches(state: EditorState, active: Set<number>, from: number, to: number): boolean {
  const first = state.doc.lineAt(from).number;
  const last = state.doc.lineAt(to).number;
  for (let n = first; n <= last; n++) {
    if (active.has(n)) return true;
  }
  return false;
}

/** Every line of `[from, to]` gets `className`. */
function eachLine(
  state: EditorState,
  from: number,
  to: number,
  className: string,
  out: PreviewSpan[],
): void {
  const first = state.doc.lineAt(from).number;
  const last = state.doc.lineAt(to).number;
  for (let n = first; n <= last; n++) {
    out.push({ kind: "line", from: state.doc.line(n).from, className });
  }
}

const HEADING = /^(?:ATX|Setext)Heading([1-6])$/;
const INLINE_STYLE: Record<string, string> = {
  StrongEmphasis: "cm-md-strong",
  Emphasis: "cm-md-em",
  Strikethrough: "cm-md-strike",
  InlineCode: "cm-md-code",
};

/**
 * What to draw for the part of the document between `from` and `to`.
 *
 * The syntax tree may not cover the whole document yet (the parser works in
 * the background on long ones); what it has not reached is drawn as plain
 * source until it has.
 */
export function previewSpans(state: EditorState, from = 0, to = state.doc.length): PreviewSpan[] {
  const out: PreviewSpan[] = [];
  const active = activeLines(state);
  const doc = state.doc;

  const matterEnd = frontMatterEnd(state);
  if (matterEnd > 0) {
    eachLine(state, 0, matterEnd, "cm-md-frontmatter", out);
  }

  syntaxTree(state).iterate({
    from,
    to,
    enter: (node) => {
      // YAML is not Markdown: its `---` would read as a rule and its last
      // key as a setext heading.
      if (node.to <= matterEnd) return false;
      const name = node.name;

      const heading = HEADING.exec(name);
      if (heading) {
        eachLine(state, node.from, node.to, `cm-md-h${heading[1]}`, out);
        return;
      }

      switch (name) {
        case "HeaderMark": {
          if (touches(state, active, node.from, node.to)) return;
          const parent = node.node.parent?.name ?? "";
          if (parent.startsWith("Setext")) {
            // The underline of a setext heading is a line of its own.
            out.push({ kind: "mark", from: node.from, to: node.to, className: "cm-md-faint" });
            return;
          }
          // `## ` — the marks and the space after them.
          const end = doc.sliceString(node.to, node.to + 1) === " " ? node.to + 1 : node.to;
          out.push({ kind: "hide", from: node.from, to: end });
          return;
        }

        case "StrongEmphasis":
        case "Emphasis":
        case "Strikethrough":
        case "InlineCode":
          out.push({ kind: "mark", from: node.from, to: node.to, className: INLINE_STYLE[name] });
          return;

        case "EmphasisMark":
        case "StrikethroughMark":
          if (!touches(state, active, node.from, node.to)) {
            out.push({ kind: "hide", from: node.from, to: node.to });
          }
          return;

        case "CodeMark": {
          const parent = node.node.parent?.name;
          if (parent === "InlineCode") {
            if (!touches(state, active, node.from, node.to)) {
              out.push({ kind: "hide", from: node.from, to: node.to });
            }
          } else {
            out.push({ kind: "mark", from: node.from, to: node.to, className: "cm-md-faint" });
          }
          return;
        }

        case "Link": {
          const first = node.node.firstChild;
          const inline =
            first?.name === "LinkMark" && doc.sliceString(first.from, first.to) === "[";
          // An autolink (`<https://…>`) is nothing but its URL, and hiding
          // that would hide the link; it is only coloured.
          if (!first || !inline) {
            out.push({ kind: "mark", from: node.from, to: node.to, className: "cm-md-link" });
            return false;
          }
          let close = first.nextSibling;
          while (
            close &&
            !(close.name === "LinkMark" && doc.sliceString(close.from, close.to) === "]")
          ) {
            close = close.nextSibling;
          }
          const textEnd = close ? close.from : node.to;
          if (!touches(state, active, node.from, node.to)) {
            // `[` before the text, and `](url "title")` or `][ref]` after it.
            out.push({ kind: "hide", from: first.from, to: first.to });
            if (close) out.push({ kind: "hide", from: close.from, to: node.to });
          }
          out.push({ kind: "mark", from: first.to, to: textEnd, className: "cm-md-link" });
          return false;
        }

        case "Image":
          out.push({ kind: "mark", from: node.from, to: node.to, className: "cm-md-faint" });
          return false;

        case "FencedCode":
        case "CodeBlock":
          eachLine(state, node.from, node.to, "cm-md-codeblock", out);
          return;

        case "CodeInfo":
          out.push({ kind: "mark", from: node.from, to: node.to, className: "cm-md-faint" });
          return;

        case "Blockquote":
          eachLine(state, node.from, node.to, "cm-md-quote", out);
          return;

        case "QuoteMark":
          out.push({ kind: "mark", from: node.from, to: node.to, className: "cm-md-faint" });
          return;

        case "ListMark": {
          const mark = doc.sliceString(node.from, node.to);
          const bullet = mark === "-" || mark === "*" || mark === "+";
          if (bullet && !touches(state, active, node.from, node.to)) {
            out.push({ kind: "bullet", from: node.from, to: node.to });
          } else {
            out.push({ kind: "mark", from: node.from, to: node.to, className: "cm-md-listmark" });
          }
          return;
        }

        case "TaskMarker": {
          const text = doc.sliceString(node.from, node.to);
          if (!touches(state, active, node.from, node.to)) {
            out.push({
              kind: "task",
              from: node.from,
              to: node.to,
              checked: /\[[xX]\]/.test(text),
            });
          }
          return;
        }

        case "HorizontalRule":
          eachLine(state, node.from, node.to, "cm-md-hr", out);
          if (!touches(state, active, node.from, node.to)) {
            out.push({
              kind: "mark",
              from: node.from,
              to: node.to,
              className: "cm-md-hidden-text",
            });
          }
          return;

        case "Table":
          eachLine(state, node.from, node.to, "cm-md-table", out);
          return false;

        case "HTMLBlock":
          eachLine(state, node.from, node.to, "cm-md-html", out);
          return false;

        default:
          return;
      }
    },
  });

  return out;
}

class BulletWidget extends WidgetType {
  eq(): boolean {
    return true;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "cm-md-bullet";
    span.textContent = "•";
    return span;
  }
}

/** A task's `[ ]`, drawn as a checkbox that edits the source when clicked. */
class TaskWidget extends WidgetType {
  constructor(
    readonly checked: boolean,
    readonly from: number,
  ) {
    super();
  }

  eq(other: TaskWidget): boolean {
    return other.checked === this.checked && other.from === this.from;
  }

  toDOM(view: EditorView): HTMLElement {
    const box = document.createElement("input");
    box.type = "checkbox";
    box.className = "cm-md-task";
    box.checked = this.checked;
    box.addEventListener("mousedown", (e) => {
      e.preventDefault();
      // Where the marker is now, not where it was when this was drawn.
      const pos = view.posAtDOM(box);
      const current = view.state.doc.sliceString(pos, pos + 3);
      if (!/^\[[ xX]\]$/.test(current)) return;
      view.dispatch({
        changes: { from: pos + 1, to: pos + 2, insert: current[1] === " " ? "x" : " " },
        userEvent: "input.toggle-task",
      });
    });
    return box;
  }

  ignoreEvent(): boolean {
    return false;
  }
}

const hidden = Decoration.replace({});
const bullet = Decoration.replace({ widget: new BulletWidget() });

function decorate(view: EditorView): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  const lineClasses = new Map<number, Set<string>>();

  for (const { from, to } of view.visibleRanges) {
    for (const span of previewSpans(view.state, from, to)) {
      switch (span.kind) {
        case "line": {
          const set = lineClasses.get(span.from) ?? new Set<string>();
          set.add(span.className);
          lineClasses.set(span.from, set);
          break;
        }
        case "mark":
          if (span.to > span.from) {
            ranges.push(Decoration.mark({ class: span.className }).range(span.from, span.to));
          }
          break;
        case "hide":
          if (span.to > span.from) ranges.push(hidden.range(span.from, span.to));
          break;
        case "bullet":
          ranges.push(bullet.range(span.from, span.to));
          break;
        case "task":
          ranges.push(
            Decoration.replace({ widget: new TaskWidget(span.checked, span.from) }).range(
              span.from,
              span.to,
            ),
          );
          break;
      }
    }
  }

  for (const [from, classes] of lineClasses) {
    ranges.push(Decoration.line({ class: [...classes].join(" ") }).range(from));
  }

  // Replacements that overlap — a hidden mark inside a hidden link title —
  // are not allowed in one set; the first of two overlapping ones wins.
  ranges.sort((a, b) => a.from - b.from || a.value.startSide - b.value.startSide);
  const kept: Range<Decoration>[] = [];
  let replacedTo = -1;
  for (const range of ranges) {
    const replaces = range.from !== range.to && range.value.point;
    if (replaces) {
      if (range.from < replacedTo) continue;
      replacedTo = range.to;
    }
    kept.push(range);
  }
  return Decoration.set(kept, true);
}

/** The live preview, as an editor extension. */
export function livePreview(): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;

      constructor(view: EditorView) {
        this.decorations = decorate(view);
      }

      update(update: ViewUpdate): void {
        if (
          update.docChanged ||
          update.viewportChanged ||
          update.selectionSet ||
          syntaxTree(update.startState) !== syntaxTree(update.state)
        ) {
          this.decorations = decorate(update.view);
        }
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}
