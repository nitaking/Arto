/**
 * The source editor in the page.
 *
 * The app renders an empty `<div class="editor-host">` beside the preview
 * (`crates/arto/src/components/content/editor_pane.rs`) and hands it the text
 * to edit through [`mount`]. The editor is CodeMirror with a live preview
 * (`./live-preview.ts`): what it holds is always the Markdown source, and it
 * is only drawn as what it means.
 *
 * Everything that decides what happens to the text — saving, conflicts,
 * drafts — is on the Rust side. This file edits, reports every change back,
 * and keeps the editor and the preview looking at the same place:
 *
 * - Moving the caret brings the block it is in into view in the preview.
 * - Double-clicking a block in the preview puts the caret on its source.
 *
 * # Modes
 *
 * `rich` draws the source with the live preview and stands alone; `source`
 * shows every character as written, beside the rendered page. [`setMode`]
 * switches in the same view — the text, caret, scroll and undo history stay —
 * and the two editor-to-page links above only work while there is a page.
 *
 * # Keys
 *
 * Every mount carries the key the app gave it (its session and generation),
 * and every change is reported with nothing but its text: the app's side of
 * the channel was opened for that key alone. [`snapshot`] is how the app
 * asks for the text right before a save, so the last keystroke is never
 * still on its way when the file is written.
 */

import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { drawSelection, EditorView, keymap } from "@codemirror/view";

import { livePreview } from "./live-preview";
import { readSourceRange } from "./source-range";

/** How the document is shown while it is edited; see the module docs. */
export type EditorMode = "rich" | "source";

interface Mounted {
  view: EditorView;
  key: string;
}

let mounted: Mounted | null = null;
let currentMode: EditorMode = "rich";
const modeCompartment = new Compartment();

function modeExtension(mode: EditorMode): Extension {
  return [
    mode === "rich" ? livePreview() : [],
    EditorView.editorAttributes.of({ class: `cm-mode-${mode}` }),
  ];
}
let followTimer: number | undefined;

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

function scheduleFollow(view: EditorView): void {
  window.clearTimeout(followTimer);
  if (currentMode !== "source") return;
  followTimer = window.setTimeout(() => {
    const head = view.state.selection.main.head;
    followLine(view.state.doc.lineAt(head).number);
  }, 160);
}

/**
 * Put `text` into an editor in `host`, replacing any editor already mounted.
 * `onChange` hears the whole text after every change.
 */
export function mount(
  host: HTMLElement,
  text: string,
  key: string,
  mode: EditorMode,
  onChange: (text: string) => void,
): void {
  unmount();
  currentMode = mode;
  const view = new EditorView({
    parent: host,
    state: EditorState.create({
      doc: text,
      extensions: [
        history(),
        drawSelection(),
        markdown({ base: markdownLanguage }),
        modeCompartment.of(modeExtension(mode)),
        EditorView.lineWrapping,
        keymap.of([indentWithTab, ...defaultKeymap, ...historyKeymap]),
        EditorView.contentAttributes.of({
          spellcheck: "false",
          autocorrect: "off",
          autocapitalize: "off",
          "aria-label": "Markdown source",
        }),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) onChange(update.state.doc.toString());
          if (update.docChanged || update.selectionSet) scheduleFollow(update.view);
        }),
      ],
    }),
  });
  mounted = { view, key };
  view.focus();
}

/** Draw the editor for `mode`, keeping everything else about it. */
export function setMode(mode: EditorMode): void {
  if (mode === currentMode) return;
  currentMode = mode;
  if (!mounted || !mounted.view.dom.isConnected) return;
  const { view } = mounted;
  view.dispatch({
    effects: [
      modeCompartment.reconfigure(modeExtension(mode)),
      // The lines change height between the two, so the caret is put back
      // in view rather than left wherever the reflow puts it.
      EditorView.scrollIntoView(view.state.selection.main.head, { y: "center" }),
    ],
  });
  view.focus();
  if (mode === "source") scheduleFollow(view);
}

/** Which mode the editor is drawn in. */
export function mode(): EditorMode {
  return currentMode;
}

/** Take the editor down, if one is mounted. */
export function unmount(): void {
  mounted?.view.destroy();
  mounted = null;
}

/** The editor's text as it is this moment, and the key it was mounted with. */
export function snapshot(): { key: string; text: string } | null {
  // A view whose host the app has already taken away is not the editor.
  if (!mounted || !mounted.view.dom.isConnected) return null;
  return { key: mounted.key, text: mounted.view.state.doc.toString() };
}

/** Put the caret at the start of `line` and scroll the editor to it. */
export function revealLine(line: number): void {
  if (!mounted || !mounted.view.dom.isConnected) return;
  const { view } = mounted;
  const target = view.state.doc.line(Math.min(Math.max(line, 1), view.state.doc.lines));
  view.dispatch({
    selection: { anchor: target.from },
    effects: EditorView.scrollIntoView(target.from, { y: "start", yMargin: 80 }),
  });
  view.focus();
}

function handlePreviewDoubleClick(e: MouseEvent): void {
  if (!mounted || currentMode !== "source") return;
  const target = e.target instanceof Element ? e.target : null;
  if (!target || target.closest(".editor-host")) return;
  const block = target.closest<HTMLElement>(".markdown-body [data-source-range]");
  const range = block ? readSourceRange(block) : null;
  if (!range) return;
  e.preventDefault();
  window.getSelection()?.removeAllRanges();
  revealLine(range.start.line);
}

/** Install the preview's double-click. Call once. */
export function setup(): void {
  document.addEventListener("dblclick", handlePreviewDoubleClick);
}
