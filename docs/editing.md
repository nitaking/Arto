# Editing

Arto can edit the document it is showing. `Cmd+E` (or the pencil in the
header, or **File → Edit Document**) opens the Markdown source beside the
page; the page becomes a live preview of it. `Cmd+S` saves, and **Done**
(or `Cmd+E` again) goes back to reading.

The editor works on the source text exactly as it is on disk — nothing goes
through the rendered HTML — so a save changes only what was typed.

## Moving between source and page

- Moving the caret brings the block it is in into view in the preview.
- Double-clicking a block in the preview puts the caret on its first source
  line.
- `Tab` / `Shift+Tab` indent and outdent the selected lines; `Return` inside a
  list item starts the next one (and ends the list on an empty item).
- Links, diagrams, formulas, the contents gutter and find all work on the
  preview while editing.

## What a save guarantees

| Guarantee | How |
| --- | --- |
| Line endings and BOM are preserved | They are recorded when the file is opened and restored on save. A file that mixes line endings is announced before it is saved, because it will be written with one kind throughout. |
| A save never overwrites a version it has not seen | Each edit remembers the SHA-256 of the file it started from. The file is read again right before writing; if it has changed (another editor, `git checkout`, a sync client) or disappeared, nothing is written and the editor asks which version to keep. |
| Nothing is written when nothing changed | Saving text identical to the file leaves the file, and its modification time, alone. |
| A save is verified | The file is synced and read back; a mismatch is reported as a failure. |
| The file keeps its identity | The file is written in place, so hard links, permissions, extended attributes and file watchers keep working. |
| Unsaved edits are never lost | The buffer is kept as a draft in Arto's data directory while it is unsaved — before every save, as you type, and when the window closes or the document changes. The next edit of that file restores the draft; if the file changed since, that is shown as a conflict. |
| One file, one editor | A file already being edited in another Arto window is not opened for editing twice; that window is brought forward instead. |

When the file changes on disk while you are editing:

- with no unsaved changes, the editor quietly takes the new version;
- with unsaved changes, a banner offers **Use the file on disk**,
  **Keep my edits** (the next save overwrites the file) or **Copy my text** (to
  merge by hand). Saving is blocked until one is chosen.

Drafts live in `drafts/` under Arto's data directory
(`~/Library/Application Support/arto/drafts` on macOS), one JSON file per
document.

## Keybindings

`editor.toggle` (`Cmd+E`) and `editor.save` (`Cmd+S`) are menu shortcuts in
every preset. A `mappings.json` written before these existed does not have
them; add them under `menuShortcuts`, or pick a preset again in
**Preferences → Keybindings**:

```json
{ "key": "Cmd+e", "action": "editor.toggle" },
{ "key": "Cmd+s", "action": "editor.save" }
```

## Limitations

- Only UTF-8 text files can be edited.
- **Copy as Markdown** from the preview reads the file on disk, so while there
  are unsaved changes that move lines around it may copy the saved text.
- The editor is a plain text area: no syntax highlighting.
