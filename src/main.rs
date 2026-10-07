mod flags;
mod shortcut;

use ksni::{blocking::TrayMethods, menu::*};
use serde::Deserialize;
use shortcut::Shortcut;
use std::{
    env,
    error::Error,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Sender},
    thread,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Default, Deserialize)]
struct Layouts {
    names: Vec<String>,
    current_idx: usize,
}

impl Layouts {
    fn current(&self) -> &str {
        self.names
            .get(self.current_idx)
            .map(String::as_str)
            .unwrap_or("Unknown layout")
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::KeyboardLayoutsChanged { keyboard_layouts } => *self = keyboard_layouts,
            Event::KeyboardLayoutSwitched { idx } if idx < self.names.len() => {
                self.current_idx = idx
            }
            _ => {}
        }
    }
}

#[derive(Deserialize)]
enum Event {
    KeyboardLayoutsChanged { keyboard_layouts: Layouts },
    KeyboardLayoutSwitched { idx: usize },
}

fn parse_event(line: &str) -> serde_json::Result<Option<Event>> {
    let value: serde_json::Value = serde_json::from_str(line)?;
    if value.get("KeyboardLayoutsChanged").is_some()
        || value.get("KeyboardLayoutSwitched").is_some()
    {
        serde_json::from_value(value).map(Some)
    } else {
        Ok(None)
    }
}

enum Action {
    Next,
    Select(usize),
    Configure,
    Configured(String),
    Enable(bool),
    Layout(Event),
    Error(String),
    Disconnected(String),
    Quit,
}

struct Tray {
    layouts: Layouts,
    shortcut: Shortcut,
    actions: Sender<Action>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }
    fn title(&self) -> String {
        self.layouts.current().into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![flags::icon(self.layouts.current())]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.title(),
            description: "Click to switch keyboard layout. Right-click for settings.".into(),
            ..Default::default()
        }
    }
    fn activate(&mut self, _: i32, _: i32) {
        let _ = self.actions.send(Action::Next);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            RadioGroup {
                selected: self.layouts.current_idx,
                options: self
                    .layouts
                    .names
                    .iter()
                    .map(|name| RadioItem {
                        label: name.replace('_', "__"),
                        ..Default::default()
                    })
                    .collect(),
                select: Box::new(|tray: &mut Self, idx| {
                    let _ = tray.actions.send(Action::Select(idx));
                }),
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Next language".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.actions.send(Action::Next);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Configure shortcut...".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.actions.send(Action::Configure);
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: format!("Enable shortcut: {}", self.shortcut.key.replace('_', "__")),
                checked: self.shortcut.enabled,
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.actions.send(Action::Enable(!tray.shortcut.enabled));
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.actions.send(Action::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn notify(kind: &str, text: String) {
    eprintln!("{text}");
    let kind = kind.to_owned();
    thread::spawn(move || {
        if let Err(error) = Command::new("zenity")
            .args([
                &kind,
                "--title=Language tray",
                "--text",
                &text,
                "--no-markup",
            ])
            .status()
        {
            eprintln!("Could not show dialog: {error}");
        }
    });
}

fn configure(key: String, actions: Sender<Action>) {
    thread::spawn(move || {
        let result = Command::new("zenity")
            .args([
                "--entry",
                "--title=Keyboard shortcut",
                "--text=Enter a niri shortcut, for example Mod+Space or Ctrl+Alt+L.",
                "--entry-text",
                &key,
            ])
            .output();
        let action = match result {
            Ok(output) if output.status.success() => {
                Action::Configured(String::from_utf8_lossy(&output.stdout).trim().into())
            }
            Ok(output) if output.status.code() == Some(1) => return, // Cancel is not an error.
            Ok(output) => Action::Error(format!(
                "Shortcut dialog failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )),
            Err(e) => Action::Error(format!("Install zenity for shortcut dialogs: {e}")),
        };
        let _ = actions.send(action);
    });
}

fn event_stream(actions: Sender<Action>) -> Result<()> {
    let socket =
        env::var_os("NIRI_SOCKET").ok_or("NIRI_SOCKET is missing. Run this app inside niri.")?;
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(b"\"EventStream\"\n")?;
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader.read_line(&mut reply)?;
    let reply: serde_json::Value = serde_json::from_str(&reply)?;
    if reply.get("Ok").is_none() {
        return Err(format!("Niri rejected event stream: {reply}").into());
    }
    thread::spawn(move || {
        for line in reader.lines() {
            let event = match line {
                Ok(line) => parse_event(&line).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            match event {
                Ok(None) => {}
                Ok(Some(event)) => {
                    if actions.send(Action::Layout(event)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = actions.send(Action::Disconnected(error));
                    return;
                }
            }
        }
        let _ = actions.send(Action::Disconnected("Niri closed the event stream".into()));
    });
    Ok(())
}

fn switch(target: &str) -> Result<()> {
    let output = Command::new("niri")
        .args(["msg", "action", "switch-layout", target])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Could not switch layout: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!(
            "Language tray for niri\n\nUsage: language-tray-indicator [--shortcut KEY | --enable-shortcut | --disable-shortcut]\n\nNo arguments: start the tray. Shortcuts are disabled by default.\nUse --shortcut Mod+Space to set a key without enabling it.\nShortcut dialogs require zenity. A StatusNotifier tray host such as Waybar is required."
        );
        return Ok(());
    }
    let dir: PathBuf = shortcut::directory()?;
    let mut settings = shortcut::load(&dir)?;
    if !args.is_empty() {
        match args.as_slice() {
            [option, key] if option == "--shortcut" => settings.key = key.clone(),
            [option] if option == "--enable-shortcut" => settings.enabled = true,
            [option] if option == "--disable-shortcut" => settings.enabled = false,
            _ => return Err("Unknown arguments. Use --help.".into()),
        }
        shortcut::save(&dir, &settings)?;
        println!("{}", shortcut::instructions(&dir));
        return Ok(());
    }
    // Create the disabled include on first launch. Never edit the user's niri config.
    shortcut::save(&dir, &settings)?;
    let output = Command::new("niri")
        .args(["msg", "--json", "keyboard-layouts"])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Could not read niri layouts: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let layouts: Layouts = serde_json::from_slice(&output.stdout)?;
    let (actions, receiver) = mpsc::channel();
    event_stream(actions.clone())?;
    let handle = Tray {
        layouts,
        shortcut: settings.clone(),
        actions: actions.clone(),
    }
    .spawn()?;
    for action in receiver {
        let result = match action {
            Action::Next => switch("next"),
            Action::Select(idx) => switch(&idx.to_string()), // Niri's layout indexes are zero-based.
            Action::Configure => {
                configure(settings.key.clone(), actions.clone());
                Ok(())
            }
            Action::Configured(key) => {
                let proposed = Shortcut {
                    key,
                    ..settings.clone()
                };
                shortcut::save(&dir, &proposed).map(|()| {
                    settings = proposed;
                    handle.update(|tray| tray.shortcut = settings.clone());
                    notify("--info", shortcut::instructions(&dir));
                })
            }
            Action::Enable(enabled) => {
                let proposed = Shortcut {
                    enabled,
                    ..settings.clone()
                };
                shortcut::save(&dir, &proposed).map(|()| {
                    settings = proposed;
                    handle.update(|tray| tray.shortcut = settings.clone());
                    if enabled {
                        notify("--info", shortcut::instructions(&dir));
                    }
                })
            }
            Action::Layout(event) => {
                handle.update(|tray| tray.layouts.apply(event));
                Ok(())
            }
            Action::Error(error) => Err(error.into()),
            Action::Disconnected(error) => return Err(error.into()),
            Action::Quit => break,
        };
        if let Err(error) = result {
            notify("--error", error.to_string());
        }
        if handle.is_closed() {
            return Err("Tray service stopped".into());
        }
    }
    handle.shutdown().wait();
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("language-tray-indicator: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_initial_layout_and_external_switches() {
        let mut layouts = Layouts::default();
        layouts.apply(serde_json::from_str(r#"{"KeyboardLayoutsChanged":{"keyboard_layouts":{"names":["English (US)","Polish","German"],"current_idx":1}}}"#).unwrap());
        assert_eq!(layouts.current(), "Polish");
        layouts.apply(serde_json::from_str(r#"{"KeyboardLayoutSwitched":{"idx":2}}"#).unwrap());
        assert_eq!(layouts.current(), "German");
        layouts.apply(Event::KeyboardLayoutSwitched { idx: 99 });
        assert_eq!(layouts.current(), "German");
        assert!(
            parse_event(r#"{"ConfigLoaded":{"failed":false}}"#)
                .unwrap()
                .is_none()
        );
        assert!(parse_event(r#"{"KeyboardLayoutSwitched":{"idx":"bad"}}"#).is_err());
        assert_eq!(layouts.current(), "German");
        layouts.apply(Event::KeyboardLayoutsChanged {
            keyboard_layouts: Layouts::default(),
        });
        assert_eq!(layouts.current(), "Unknown layout");
    }
}
