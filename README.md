# Excalidraw

[Excalidraw](https://excalidraw.com) drawings in den. A `.excalidraw` file's tab has **Preview** beside Source: click it and Excalidraw opens in your browser, on that file.

- **Draw in the browser, saved to the file:** every change is saved a moment after you stop (or at once with <kbd>Ctrl</kbd>+<kbd>S</kbd>). Opening a drawing only to look at it never changes the file.
- **Source and the drawing agree:** edit the JSON in Source and save, or change the file another way (`git checkout`, an agent), and the browser shows the change within a second. Drawing in the browser updates Source the same way.
- **One page per click:** each click on Preview opens the drawing in a new browser tab. The page works while den is open, as den serves it.
- **New Drawing** (in den's menu under Extensions, or the command palette) asks for a name and makes an empty drawing in the folder, `.excalidraw` added when you leave it out.
- **Export image** (Excalidraw's menu) saves a PNG or SVG of the drawing, named after the file.

## Settings

| Setting | |
| --- | --- |
| **Theme** | `system` (follows the system's light or dark mode), `light` or `dark` |

## Good to know

- Excalidraw is loaded from [esm.sh](https://esm.sh) (`@excalidraw/excalidraw` 0.18.1), so the first drawing you open needs the internet. Your browser keeps it cached after that.
- A file that isn't a drawing (or is broken JSON) shows what is wrong instead, and is never written over; fix it in Source and the drawing shows once it is saved.
- Unsaved edits in Source don't show in the browser until saved, and a drawing saved meanwhile doesn't replace them (den keeps unsaved edits); saving them later overwrites the drawing.

## How it works

The extension runs a small HTTP server on a free port of 127.0.0.1 (`src/server.rs`), under a random token so that no other page on the machine can read or write through it. Preview is a file view (`set_file_views`): when den sends `file_view_shown`, the extension answers with the file's page on the server (`load_file_view`), which den 0.32 and later open in the system's browser (older dens showed it in place of the editor). The page (`src/page.html`) is Excalidraw: it reads the file from the server, writes it back after each change, and asks every second whether the file changed on disk and which theme to show. It needs a den newer than 0.20, which has file views.

```sh
cargo test
cargo build --release
```

To build against a local den checkout, add a `.cargo/config.toml` (it is ignored) with:

```toml
[patch."https://github.com/patrickiel/den"]
den-extension = { path = "../den/crates/den-extension" }
```

Then run `node ../den/scripts/sideload.ts --dev .` from this folder.
