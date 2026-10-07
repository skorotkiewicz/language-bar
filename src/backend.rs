use crate::{Action, Event, Layouts, Result, x11};
use std::{
    env,
    io::{Read, Write},
    os::unix::net::UnixStream,
    process::Command,
    sync::{Arc, mpsc::Sender},
    thread,
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Niri,
    Sway,
    X11,
}

impl Kind {
    pub fn detect() -> Result<Self> {
        detect(
            env::var_os("NIRI_SOCKET").is_some(),
            env::var_os("SWAYSOCK").is_some(),
            env::var_os("WAYLAND_DISPLAY").is_some()
                || env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland"),
            env::var_os("DISPLAY").is_some(),
        )
    }
}

fn detect(niri: bool, sway: bool, wayland: bool, x11: bool) -> Result<Kind> {
    if niri {
        Ok(Kind::Niri)
    } else if sway {
        Ok(Kind::Sway)
    } else if wayland {
        Err("This Wayland compositor has no supported layout API. Supported: niri and Sway. A wlroots base alone does not expose keyboard layouts. XWayland is not a fallback.".into())
    } else if x11 {
        Ok(Kind::X11)
    } else {
        Err("No supported desktop session found".into())
    }
}

pub enum Backend {
    Niri,
    Sway,
    X11(Arc<x11::Keyboard>),
}

impl Backend {
    pub fn connect(kind: Kind) -> Result<Self> {
        Ok(match kind {
            Kind::Niri => Self::Niri,
            Kind::Sway => Self::Sway,
            Kind::X11 => Self::X11(Arc::new(x11::Keyboard::connect()?)),
        })
    }

    pub fn layouts(&self) -> Result<Layouts> {
        match self {
            Self::Niri => {
                let output = Command::new("niri")
                    .args(["msg", "--json", "keyboard-layouts"])
                    .output()?;
                if !output.status.success() {
                    return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
                }
                Ok(serde_json::from_slice(&output.stdout)?)
            }
            Self::Sway => sway_layouts(),
            Self::X11(keyboard) => keyboard.layouts(),
        }
    }

    pub fn switch(&self, target: &str) -> Result<()> {
        match self {
            Self::Niri => {
                let output = Command::new("niri")
                    .args(["msg", "action", "switch-layout", target])
                    .output()?;
                if !output.status.success() {
                    return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
                }
                Ok(())
            }
            Self::Sway => sway_command(&format!("input type:keyboard xkb_switch_layout {target}")),
            Self::X11(keyboard) => keyboard.switch(target),
        }
    }

    pub fn watch(&self, actions: Sender<Action>) -> Result<()> {
        match self {
            Self::Niri => crate::event_stream(actions),
            Self::Sway => {
                let mut stream = sway_socket()?;
                send(&mut stream, 2, br#"["input"]"#)?;
                let (_, reply) = receive(&mut stream)?;
                if serde_json::from_slice::<serde_json::Value>(&reply)?["success"] != true {
                    return Err("Sway rejected the input subscription".into());
                }
                stream.set_read_timeout(None)?;
                // Query after subscribing, closing the initial snapshot/subscription race.
                actions.send(Action::Layout(Event::KeyboardLayoutsChanged {
                    keyboard_layouts: sway_layouts()?,
                }))?;
                thread::spawn(move || {
                    loop {
                        let result = receive(&mut stream).and_then(|_| sway_layouts());
                        match result {
                            Ok(layouts) => {
                                if actions
                                    .send(Action::Layout(Event::KeyboardLayoutsChanged {
                                        keyboard_layouts: layouts,
                                    }))
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(e) => {
                                let _ = actions.send(Action::Disconnected(e.to_string()));
                                break;
                            }
                        }
                    }
                });
                Ok(())
            }
            Self::X11(keyboard) => keyboard.watch(actions),
        }
    }

    pub fn set_shortcut(&self, shortcut: &crate::shortcut::Shortcut) -> Result<()> {
        if let Self::X11(keyboard) = self {
            keyboard.set_shortcut(shortcut)?;
        }
        Ok(())
    }
}

fn sway_socket() -> Result<UnixStream> {
    let path = env::var_os("SWAYSOCK").ok_or("SWAYSOCK is missing")?;
    let socket = UnixStream::connect(path)?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(socket)
}

fn send(stream: &mut UnixStream, kind: u32, payload: &[u8]) -> Result<()> {
    stream.write_all(b"i3-ipc")?;
    stream.write_all(&(u32::try_from(payload.len())?).to_le_bytes())?;
    stream.write_all(&kind.to_le_bytes())?;
    stream.write_all(payload)?;
    Ok(())
}

fn receive(stream: &mut UnixStream) -> Result<(u32, Vec<u8>)> {
    let mut header = [0; 14];
    stream.read_exact(&mut header)?;
    if &header[..6] != b"i3-ipc" {
        return Err("Invalid Sway IPC header".into());
    }
    let len = u32::from_le_bytes(header[6..10].try_into()?) as usize;
    if len > 16 * 1024 * 1024 {
        return Err("Sway IPC message is too large".into());
    }
    let kind = u32::from_le_bytes(header[10..].try_into()?);
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload)?;
    Ok((kind, payload))
}

fn sway_request(kind: u32, payload: &[u8]) -> Result<serde_json::Value> {
    let mut socket = sway_socket()?;
    send(&mut socket, kind, payload)?;
    let (reply_kind, reply) = receive(&mut socket)?;
    if reply_kind != kind {
        return Err("Unexpected Sway IPC reply".into());
    }
    Ok(serde_json::from_slice(&reply)?)
}

pub fn sway_command(command: &str) -> Result<()> {
    let reply = sway_request(0, command.as_bytes())?;
    let replies = reply.as_array().ok_or("Invalid Sway command response")?;
    if replies.is_empty() || replies.iter().any(|r| r["success"] != true) {
        return Err(format!("Sway command failed: {reply}").into());
    }
    Ok(())
}

fn sway_layouts() -> Result<Layouts> {
    sway_inputs(&sway_request(100, b"")?)
}

fn sway_inputs(inputs: &serde_json::Value) -> Result<Layouts> {
    // ponytail: show the first keyboard and switch all; use per-device trays if layouts differ.
    let input = inputs
        .as_array()
        .ok_or("Invalid Sway input response")?
        .iter()
        .find(|i| {
            i["type"] == "keyboard"
                && i["xkb_layout_names"]
                    .as_array()
                    .is_some_and(|n| !n.is_empty())
        })
        .ok_or("Sway has no keyboard with configured layouts")?;
    let names: Vec<String> = serde_json::from_value(input["xkb_layout_names"].clone())?;
    let current_idx = input["xkb_active_layout_index"]
        .as_u64()
        .map(|i| i as usize)
        .or_else(|| {
            names
                .iter()
                .position(|n| Some(n.as_str()) == input["xkb_active_layout_name"].as_str())
        })
        .ok_or("Sway did not report the active keyboard layout")?;
    if current_idx >= names.len() {
        return Err("Sway reported an invalid layout index".into());
    }
    Ok(Layouts { names, current_idx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sessions_without_mistaking_xwayland_for_x11() {
        assert_eq!(detect(true, false, true, true).unwrap(), Kind::Niri);
        assert_eq!(detect(false, true, true, true).unwrap(), Kind::Sway);
        assert_eq!(detect(false, false, false, true).unwrap(), Kind::X11);
        assert!(detect(false, false, true, true).is_err());
        let inputs = serde_json::json!([
            {"type":"pointer"},
            {"type":"keyboard","xkb_layout_names":["English (US)","Japanese"],"xkb_active_layout_index":1}
        ]);
        assert_eq!(sway_inputs(&inputs).unwrap().current(), "Japanese");
        assert!(sway_inputs(&serde_json::json!([])).is_err());
        let (mut client, mut server) = UnixStream::pair().unwrap();
        send(&mut client, 100, b"test").unwrap();
        assert_eq!(receive(&mut server).unwrap(), (100, b"test".to_vec()));
        client.write_all(b"invalid-header").unwrap();
        assert!(receive(&mut server).is_err());
    }
}
