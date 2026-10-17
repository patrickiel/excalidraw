//! Excalidraw for den: a `.excalidraw` file's tab offers Preview beside
//! Source, a file view whose page is Excalidraw itself, drawing straight
//! into the file.
//!
//! The page is served by a small HTTP server of the extension's own
//! (`server.rs`), on a free port of 127.0.0.1 and under a path no other page
//! can guess. It loads Excalidraw from esm.sh, reads the file through the
//! server and writes each change back; it asks every second whether the file
//! changed on disk (saved in Source, `git checkout`) and shows that. The
//! extension itself only offers the view, points each tab's page at its file,
//! and makes new drawings, so it needs no worker thread.

mod server;

use std::io::{ErrorKind, Write};
use std::path::Path;

use den_extension::view::{Field, Prompt};
use den_extension::{Context, Extension, FileView, Host, events, register};
use serde_json::Value;

use crate::server::Server;

/// The id of the Preview file view.
const VIEW: &str = "preview";
/// The New Drawing command's id, and its prompt's.
const NEW: &str = "new";
/// The files Preview shows.
const PATTERN: &str = r"(?i)\.excalidraw$";
/// What a new drawing starts as: what Excalidraw saves for an empty canvas.
const EMPTY: &str = r##"{
  "type": "excalidraw",
  "version": 2,
  "source": "https://excalidraw.com",
  "elements": [],
  "appState": {
    "gridSize": 20,
    "gridStep": 5,
    "gridModeEnabled": false,
    "viewBackgroundColor": "#ffffff"
  },
  "files": {}
}
"##;

/// The Theme setting: `system`, `light` or `dark`.
fn theme(settings: &Value) -> &str {
    settings["theme"].as_str().unwrap_or("system")
}

/// The file a New Drawing named `name` goes to, relative to the folder:
/// `.excalidraw` added when it has none. None for a blank name.
fn drawing_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(if name.to_lowercase().ends_with(".excalidraw") {
        name.to_string()
    } else {
        format!("{name}.excalidraw")
    })
}

struct Excalidraw {
    host: Host,
    /// None when it couldn't start (logged); then nothing is offered.
    server: Option<Server>,
}

impl Extension for Excalidraw {
    fn activate(host: Host, context: Context) -> Self {
        let settings = Value::Object(context.settings);
        let server = Server::start(theme(&settings))
            .inspect_err(|err| host.log(format!("cannot start the page server: {err}")))
            .ok();
        Excalidraw { host, server }
    }

    fn event(&mut self, name: &str, data: Value) {
        let Some(server) = &self.server else {
            return;
        };
        let text = |key: &str| data[key].as_str().unwrap_or_default();
        let root = text("root");
        match name {
            events::WORKSPACE_OPENED => {
                let views = [FileView {
                    id: VIEW.into(),
                    label: "Preview".into(),
                    tooltip: "Draw it in Excalidraw".into(),
                    file_pattern: PATTERN.into(),
                }];
                if let Err(err) = self.host.set_file_views(root, &views) {
                    self.host.log(format!("set_file_views: {err}"));
                }
            }
            events::FILE_VIEW_SHOWN if text("view") == VIEW => {
                let path = text("path");
                let url = server.page_url(Path::new(path));
                // A page already showing it keeps its undo history; one from
                // before a restart points at a server that is gone.
                if data["url"].as_str() != Some(url.as_str())
                    && let Err(err) = self.host.load_file_view(root, path, VIEW, &url)
                {
                    self.host.log(format!("load_file_view: {err}"));
                }
            }
            events::SETTINGS_CHANGED => server.set_theme(theme(&data["settings"])),
            events::COMMAND if text("id") == NEW => {
                let prompt = Prompt {
                    id: NEW.into(),
                    title: "New Drawing".into(),
                    message: "Its name, in this folder or a folder under it.".into(),
                    fields: vec![Field {
                        required: true,
                        ..Field::text("name", "Name", "drawing.excalidraw")
                    }],
                    ok: "Create".into(),
                    ..Default::default()
                };
                if let Err(err) = self.host.prompt(root, &prompt) {
                    self.host.log(format!("prompt: {err}"));
                }
            }
            events::PROMPT_ANSWERED if text("id") == NEW => {
                if let Some(name) = data["values"]["name"].as_str().and_then(drawing_name) {
                    self.create(root, &name);
                }
            }
            _ => {}
        }
    }
}

impl Excalidraw {
    /// Make drawing `name` under `root`, empty, and open it; one that is
    /// there already is only opened.
    fn create(&self, root: &str, name: &str) {
        let path = Path::new(root).join(name);
        let made = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::File::create_new(&path))
            .and_then(|mut file| file.write_all(EMPTY.as_bytes()));
        match made {
            Err(err) if err.kind() != ErrorKind::AlreadyExists => {
                self.host.toast(format!("Cannot create {name}: {err}"));
                return;
            }
            _ => {}
        }
        if let Err(err) = self.host.open_file(root, &path.to_string_lossy(), None) {
            self.host.log(format!("open_file: {err}"));
        }
    }
}

register!(Excalidraw);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_get_the_extension_once() {
        assert_eq!(drawing_name(" plan ").as_deref(), Some("plan.excalidraw"));
        assert_eq!(drawing_name("docs/Plan.Excalidraw").as_deref(), Some("docs/Plan.Excalidraw"));
        assert_eq!(drawing_name("  "), None);
    }

    #[test]
    fn preview_matches_drawings_only() {
        let pattern = regex::Regex::new(PATTERN).unwrap();
        assert!(pattern.is_match(r"C:\work\Plan.excalidraw"));
        assert!(pattern.is_match("/work/docs/arch.EXCALIDRAW"));
        assert!(!pattern.is_match("/work/arch.excalidraw.svg"));
        assert!(!pattern.is_match("/work/excalidraw/notes.md"));
    }

    #[test]
    fn a_new_drawing_is_an_empty_scene() {
        let scene: Value = serde_json::from_str(EMPTY).unwrap();
        assert_eq!(scene["type"], "excalidraw");
        assert_eq!(scene["elements"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn manifest_agrees() {
        let manifest = den_extension::Manifest::parse(include_str!("../extension.json")).unwrap();
        assert_eq!(manifest.id, env!("CARGO_PKG_NAME"));
        assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
        assert!(manifest.commands.iter().any(|command| command.id == NEW));
        let defaults = Value::Object(manifest.settings_values(&serde_json::Map::new()));
        assert_eq!(theme(&defaults), "system");
    }
}
