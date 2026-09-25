/**
 * The source editor's behaviour in the page.
 *
 * The editor is a plain `<textarea class="editor-input">` that the app
 * renders beside the preview (`crates/arto/src/components/content/editor_pane.rs`).
 * Everything that decides what happens to the text — saving, conflicts,
 * drafts — is on the Rust side; this file only makes the textarea pleasant to
 * write Markdown in and keeps it and the preview looking at the same place:
 *
 * - Tab and Shift+Tab indent and outdent the lines under the selection.
 * - Return inside a list item starts the next item; Return on an empty item
 *   ends the list.
 * - Moving the caret brings the block it is in into view in the preview.
 * - Double-clicking a block in the preview puts the caret on its source.
 *
 * Every edit goes through `document.execCommand("insertText")`, so each one
 * is a single step on the textarea's own undo stack and fires the `input`
 * event the app listens to, exactly as typing would.
 *
 * Listeners are delegated from the document, so there is nothing to attach
 * when the app mounts a new textarea.
 */

import { readSourceRange } from "./source-range";

const INDENT = "  ";

/** 1-based line number of `offset` in `text`. */
export function lineAt(text: string, offset: number): number {
  let line = 1;
  const end = Math.min(offset, text.length);
  for (let i = 0; i < end; i++) {
    if (text.charCodeAt(i) === 10) line++;
  }
  return line;
}

/** Offset of the start of 1-based `line` in `text`, clamped to the text. */
export function offsetOfLine(text: string, line: number): number {
  if (line <= 1) return 0;
  let seen = 1;
  for (let i = 0; i < text.length; i++) {
    if (text.charCodeAt(i) === 10) {
      seen++;
      if (seen === line) return i + 1;
    }
  }
  return text.length;
}

const LIST_ITEM = /^(\s*)(?:([-*+])|(\d+)([.)]))(\s+)(\[[ xX]\]\s+)?/;

/**
 * What Return should insert after `line`, the text of the line before the
 * caret.
 *
 * - `null`: not a list item; let Return be Return.
 * - `{ end: true }`: an item with nothing in it; Return ends the list, so the
 *   marker is removed rather than repeated.
 * - `{ insert }`: the marker for the next item, after the newline.
 */
export function continueList(line: string): { insert: string } | { end: true } | null {
  const match = LIST_ITEM.exec(line);
  if (!match) return null;
  const [whole, indent, bullet, number, delimiter, gap, task] = match;
  if (line.slice(whole.length).trim() === "") return { end: true };
  const marker = bullet ?? `${Number(number) + 1}${delimiter}`;
  const checkbox = task ? "[ ] " : "";
  return { insert: `\n${indent}${marker}${gap}${checkbox}` };
}

const FENCE = /^\s{0,3}(`{3,}|~{3,})/;

/**
 * Whether `offset` is inside a fenced code block or the front matter, where a
 * line that looks like a list item is code or YAML and Return is just Return.
 */
export function insideLiteralBlock(text: string, offset: number): boolean {
  const lines = text.slice(0, offset).split("\n");
  // The caret's own line is the last one; only the lines above it can open
  // a block that it is inside.
  lines.pop();
  let fence: string | null = null;
  let frontMatter = lines[0] === "---";
  for (let i = frontMatter ? 1 : 0; i < lines.length; i++) {
    const line = lines[i];
    if (frontMatter) {
      if (line === "---" || line === "...") frontMatter = false;
      continue;
    }
    const match = FENCE.exec(line);
    if (!match) continue;
    const marker = match[1];
    if (fence === null) {
      fence = marker;
    } else if (marker[0] === fence[0] && marker.length >= fence.length) {
      fence = null;
    }
  }
  return frontMatter || fence !== null;
}

/** Indent every line of `block` by one step. */
export function indentBlock(block: string): string {
  return block
    .split("\n")
    .map((line) => (line.length > 0 ? INDENT + line : line))
    .join("\n");
}

/** Remove up to one step of indentation from every line of `block`. */
export function outdentBlock(block: string): string {
  return block
    .split("\n")
    .map((line) => {
      if (line.startsWith(INDENT)) return line.slice(INDENT.length);
      if (line.startsWith("\t") || line.startsWith(" ")) return line.slice(1);
      return line;
    })
    .join("\n");
}

function isEditor(target: EventTarget | null): target is HTMLTextAreaElement {
  return target instanceof HTMLTextAreaElement && target.classList.contains("editor-input");
}

function editor(): HTMLTextAreaElement | null {
  return document.querySelector<HTMLTextAreaElement>("textarea.editor-input");
}

/** Replace `[start, end)` with `text` as one undoable edit. */
function replaceRange(area: HTMLTextAreaElement, start: number, end: number, text: string): void {
  area.setSelectionRange(start, end);
  // `execCommand` is deprecated but still the only way to edit a textarea
  // that keeps the browser's undo history; assigning `value` wipes it.
  if (!document.execCommand("insertText", false, text)) {
    area.setRangeText(text, start, end, "end");
    area.dispatchEvent(new Event("input", { bubbles: true }));
  }
}

function handleTab(area: HTMLTextAreaElement, outdent: boolean): void {
  const { value, selectionStart, selectionEnd } = area;
  const lineStart = value.lastIndexOf("\n", selectionStart - 1) + 1;
  const multiLine = value.slice(selectionStart, selectionEnd).includes("\n");

  if (!outdent && !multiLine) {
    replaceRange(area, selectionStart, selectionEnd, INDENT);
    return;
  }

  // Whole lines: from the start of the first to the end of the last, not
  // counting a selection that ends at the very start of a line.
  const lastChar = selectionEnd > selectionStart ? selectionEnd - 1 : selectionEnd;
  const nextBreak = value.indexOf("\n", lastChar);
  const lineEnd = nextBreak === -1 ? value.length : nextBreak;
  const block = value.slice(lineStart, lineEnd);
  const changed = outdent ? outdentBlock(block) : indentBlock(block);
  if (changed === block) return;

  replaceRange(area, lineStart, lineEnd, changed);
  if (multiLine) {
    area.setSelectionRange(lineStart, lineStart + changed.length);
  } else {
    const moved = Math.max(selectionStart - (block.length - changed.length), lineStart);
    area.setSelectionRange(moved, moved);
  }
}

function handleReturn(area: HTMLTextAreaElement): boolean {
  const { value, selectionStart, selectionEnd } = area;
  if (selectionStart !== selectionEnd) return false;
  if (insideLiteralBlock(value, selectionStart)) return false;
  const lineStart = value.lastIndexOf("\n", selectionStart - 1) + 1;
  const next = continueList(value.slice(lineStart, selectionStart));
  if (!next) return false;
  if ("end" in next) {
    // Only an item that is empty on both sides of the caret ends the list;
    // Return just after the marker of `- text` is an ordinary line break.
    const breakAt = value.indexOf("\n", selectionStart);
    const after = value.slice(selectionStart, breakAt === -1 ? value.length : breakAt);
    if (after.trim() !== "") return false;
    replaceRange(area, lineStart, selectionStart, "");
  } else {
    replaceRange(area, selectionStart, selectionStart, next.insert);
  }
  return true;
}

function handleKeydown(e: KeyboardEvent): void {
  if (!isEditor(e.target)) return;
  // The input method owns the key while it is composing (Japanese input
  // confirms a conversion with Return).
  if (e.isComposing || e.keyCode === 229) return;
  if (e.metaKey || e.ctrlKey || e.altKey) return;

  if (e.key === "Tab") {
    e.preventDefault();
    handleTab(e.target, e.shiftKey);
  } else if (e.key === "Enter" && !e.shiftKey) {
    if (handleReturn(e.target)) e.preventDefault();
  }
}

// ---------------------------------------------------------------------------
// Keeping the editor and the preview on the same place

let followTimer: number | undefined;
let lastFollowedLine = 0;

/** The preview's top-level block that `line` of the source falls in. */
function blockForLine(line: number): HTMLElement | null {
  const body = document.querySelector(".markdown-body");
  if (!body) return null;
  let best: HTMLElement | null = null;
  for (const block of body.querySelectorAll<HTMLElement>(":scope > [data-source-range]")) {
    const range = readSourceRange(block);
    if (!range) continue;
    if (range.start.line > line) break;
    best = block;
    if (range.end.line >= line) break;
  }
  return best;
}

/** Bring the preview block for `line` into view, unless it already is. */
export function followLine(line: number): void {
  const scroller = document.querySelector<HTMLElement>(".content");
  const block = blockForLine(line);
  if (!scroller || !block) return;
  const view = scroller.getBoundingClientRect();
  const rect = block.getBoundingClientRect();
  const visible = rect.bottom > view.top + 24 && rect.top < view.bottom - 24;
  if (visible) return;
  const target = scroller.scrollTop + (rect.top - view.top) - view.height / 3;
  scroller.scrollTo({ top: Math.max(target, 0) });
}

function scheduleFollow(area: HTMLTextAreaElement): void {
  window.clearTimeout(followTimer);
  followTimer = window.setTimeout(() => {
    const line = lineAt(area.value, area.selectionStart);
    // Re-checked after an edit even on the same line: the preview may have
    // been re-rendered around it and moved.
    lastFollowedLine = line;
    followLine(line);
  }, 160);
}

/** Where `offset` sits vertically in `area`, measured with a mirror. */
function caretTop(area: HTMLTextAreaElement, offset: number): number {
  const style = getComputedStyle(area);
  const mirror = document.createElement("div");
  for (const prop of [
    "boxSizing",
    "width",
    "paddingTop",
    "paddingRight",
    "paddingBottom",
    "paddingLeft",
    "borderTopWidth",
    "borderRightWidth",
    "borderBottomWidth",
    "borderLeftWidth",
    "fontFamily",
    "fontSize",
    "fontWeight",
    "lineHeight",
    "letterSpacing",
    "tabSize",
    "whiteSpace",
    "wordBreak",
    "overflowWrap",
  ] as const) {
    mirror.style[prop] = style[prop];
  }
  mirror.style.position = "absolute";
  mirror.style.visibility = "hidden";
  mirror.style.whiteSpace = "pre-wrap";
  mirror.style.height = "auto";
  mirror.textContent = area.value.slice(0, offset);
  const marker = document.createElement("span");
  marker.textContent = "​";
  mirror.appendChild(marker);
  document.body.appendChild(mirror);
  const top = marker.offsetTop;
  mirror.remove();
  return top;
}

/** Put the caret at the start of `line` and scroll the editor to it. */
export function revealLine(line: number): void {
  const area = editor();
  if (!area) return;
  const offset = offsetOfLine(area.value, line);
  area.focus({ preventScroll: true });
  area.setSelectionRange(offset, offset);
  const top = caretTop(area, offset);
  area.scrollTop = Math.max(top - area.clientHeight / 3, 0);
  lastFollowedLine = line;
}

function handlePreviewDoubleClick(e: MouseEvent): void {
  if (!editor()) return;
  const target = e.target instanceof Element ? e.target : null;
  const block = target?.closest<HTMLElement>(".markdown-body [data-source-range]");
  const range = block ? readSourceRange(block) : null;
  if (!range) return;
  e.preventDefault();
  window.getSelection()?.removeAllRanges();
  revealLine(range.start.line);
}

/** The line the caret was last followed to; for tests and diagnostics. */
export function followedLine(): number {
  return lastFollowedLine;
}

/** Install the delegated listeners. Call once. */
export function setup(): void {
  document.addEventListener("keydown", handleKeydown);
  for (const type of ["input", "keyup", "mouseup", "focus"]) {
    document.addEventListener(
      type,
      (e) => {
        if (isEditor(e.target)) scheduleFollow(e.target);
      },
      true,
    );
  }
  document.addEventListener("dblclick", handlePreviewDoubleClick);
}
