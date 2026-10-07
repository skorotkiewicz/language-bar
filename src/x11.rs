use crate::{Action, Event, Layouts, Result, shortcut};
use std::sync::{Arc, Mutex, mpsc::Sender};
use x11rb::{
    connection::Connection,
    protocol::{
        Event as XEvent,
        xkb::{self, ConnectionExt as _, NameDetail},
        xproto::{ConnectionExt as _, GrabMode, ModMask},
    },
    rust_connection::RustConnection,
};

const DEVICE: u16 = 0x100; // XkbUseCoreKbd.

#[derive(Clone, PartialEq, Eq)]
struct Grab {
    key: u8,
    modifiers: Vec<u16>,
}

pub struct Keyboard {
    connection: RustConnection,
    root: u32,
    grab: Mutex<Option<Grab>>,
}

impl Keyboard {
    pub fn connect() -> Result<Self> {
        let (connection, screen) = x11rb::connect(None)?;
        if !connection.xkb_use_extension(1, 0)?.reply()?.supported {
            return Err("X11 server does not support XKB".into());
        }
        let root = connection.setup().roots[screen].root;
        let events = xkb::EventType::STATE_NOTIFY
            | xkb::EventType::NAMES_NOTIFY
            | xkb::EventType::NEW_KEYBOARD_NOTIFY
            | xkb::EventType::MAP_NOTIFY;
        connection
            .xkb_select_events(
                DEVICE,
                xkb::EventType::default(),
                events,
                xkb::MapPart::default(),
                xkb::MapPart::default(),
                &xkb::SelectEventsAux::default(),
            )?
            .check()?;
        connection.flush()?;
        Ok(Self {
            connection,
            root,
            grab: Mutex::new(None),
        })
    }

    pub fn layouts(&self) -> Result<Layouts> {
        let reply = self
            .connection
            .xkb_get_names(DEVICE, NameDetail::GROUP_NAMES)?
            .reply()?;
        let state = self.connection.xkb_get_state(DEVICE)?.reply()?;
        let mask = u8::from(reply.group_names);
        let mut atoms = reply.value_list.groups.unwrap_or_default().into_iter();
        let mut names = Vec::new();
        for idx in 0..4 {
            if mask >> idx == 0 {
                break;
            }
            let name = if mask & (1 << idx) != 0 {
                let atom = atoms.next().ok_or("Missing XKB group name")?;
                String::from_utf8(self.connection.get_atom_name(atom)?.reply()?.name)?
            } else {
                format!("Layout {}", idx + 1)
            };
            names.push(name);
        }
        if names.is_empty() {
            return Err("X11 has no named keyboard layouts".into());
        }
        Ok(Layouts {
            names,
            current_idx: u8::from(state.group) as usize,
        })
    }

    pub fn switch(&self, target: &str) -> Result<()> {
        let layouts = self.layouts()?;
        let idx = if target == "next" {
            (layouts.current_idx + 1) % layouts.names.len()
        } else {
            target.parse::<usize>()?
        };
        if idx >= layouts.names.len() {
            return Err("Layout index is out of range".into());
        }
        self.connection
            .xkb_latch_lock_state(
                DEVICE,
                ModMask::default(),
                ModMask::default(),
                true,
                (idx as u8).into(),
                ModMask::default(),
                false,
                0,
            )?
            .check()?;
        self.connection.flush()?;
        Ok(())
    }

    pub fn watch(self: &Arc<Self>, actions: Sender<Action>) -> Result<()> {
        let keyboard = Arc::clone(self);
        std::thread::spawn(move || {
            loop {
                let result = keyboard.connection.wait_for_event();
                let result = match result {
                    Ok(XEvent::XkbStateNotify(_))
                    | Ok(XEvent::XkbNamesNotify(_))
                    | Ok(XEvent::XkbNewKeyboardNotify(_))
                    | Ok(XEvent::XkbMapNotify(_)) => keyboard.layouts().and_then(|layouts| {
                        actions.send(Action::Layout(Event::KeyboardLayoutsChanged {
                            keyboard_layouts: layouts,
                        }))?;
                        Ok(())
                    }),
                    Ok(XEvent::KeyPress(event)) => {
                        if keyboard
                            .grab
                            .lock()
                            .unwrap()
                            .as_ref()
                            .is_some_and(|g| g.key == event.detail)
                        {
                            let _ = actions.send(Action::Next);
                        }
                        Ok(())
                    }
                    Ok(_) => Ok(()),
                    Err(e) => Err(e.into()),
                };
                if let Err(e) = result {
                    let _ = actions.send(Action::Disconnected(e.to_string()));
                    break;
                }
            }
        });
        Ok(())
    }

    fn resolve(&self, key: &str) -> Result<Grab> {
        let (modifiers, name) = shortcut::parts(key)?;
        let symbol =
            xkbcommon::xkb::keysym_from_name(name, xkbcommon::xkb::KEYSYM_CASE_INSENSITIVE).raw();
        if symbol == 0 {
            return Err(format!("Unknown XKB key: {name}").into());
        }
        let setup = self.connection.setup();
        let first = setup.min_keycode;
        let mapping = self
            .connection
            .get_keyboard_mapping(first, setup.max_keycode - first + 1)?
            .reply()?;
        let width = mapping.keysyms_per_keycode as usize;
        if width == 0 {
            return Err("X11 returned an empty keyboard mapping".into());
        }
        let symbols: Vec<_> = mapping.keysyms.chunks_exact(width).collect();
        let key = symbols
            .iter()
            .position(|s| s.contains(&symbol))
            .ok_or("Shortcut key is not present on this keyboard")? as u8
            + first;
        let mods = self.connection.get_modifier_mapping()?.reply()?;
        let per_modifier = mods.keycodes.len() / 8;
        let modifier_mask = |symbol| -> u16 {
            if per_modifier == 0 {
                return 0;
            }
            mods.keycodes
                .chunks_exact(per_modifier)
                .enumerate()
                .filter(|(_, keys)| {
                    keys.iter().any(|code| {
                        *code >= first
                            && symbols
                                .get((*code - first) as usize)
                                .is_some_and(|s| s.contains(&symbol))
                    })
                })
                .fold(0, |mask, (idx, _)| mask | (1 << idx))
        };
        let mut mask = 0;
        for modifier in modifiers {
            let bit = match modifier {
                "Shift" => 1,
                "Ctrl" => 4,
                "Alt" => modifier_mask(0xffe9) | modifier_mask(0xffea),
                "Mod" | "Super" => modifier_mask(0xffeb) | modifier_mask(0xffec),
                _ => unreachable!("shortcut modifiers were validated"),
            };
            if bit == 0 {
                return Err(format!("Modifier {modifier} is not mapped on this keyboard").into());
            }
            mask |= bit;
        }
        let caps = modifier_mask(0xffe5);
        let num = modifier_mask(0xff7f);
        let mut modifiers = vec![mask, mask | caps, mask | num, mask | caps | num];
        modifiers.sort_unstable();
        modifiers.dedup();
        Ok(Grab { key, modifiers })
    }

    fn ungrab(&self, grab: &Grab) -> Result<()> {
        for &modifier in &grab.modifiers {
            self.connection
                .ungrab_key(grab.key, self.root, modifier.into())?
                .check()?;
        }
        Ok(())
    }

    fn grab(&self, grab: &Grab) -> Result<()> {
        for &modifier in &grab.modifiers {
            if let Err(error) = self
                .connection
                .grab_key(
                    false,
                    self.root,
                    modifier.into(),
                    grab.key,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .check()
            {
                self.ungrab(grab)?;
                return Err(format!(
                    "Could not register shortcut. Another app may already use it: {error}"
                )
                .into());
            }
        }
        self.connection.flush()?;
        Ok(())
    }

    pub fn set_shortcut(&self, shortcut: &shortcut::Shortcut) -> Result<()> {
        let proposed = if shortcut.enabled {
            Some(self.resolve(&shortcut.key)?)
        } else {
            None
        };
        let mut current = self.grab.lock().unwrap();
        if *current == proposed {
            return Ok(());
        }
        if let Some(old) = current.as_ref() {
            self.ungrab(old)?;
        }
        if let Some(new) = proposed.as_ref()
            && let Err(error) = self.grab(new)
        {
            if let Some(old) = current.as_ref() {
                self.grab(old)?;
            }
            return Err(error);
        }
        *current = proposed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs an isolated X11 server with us,pl layouts and xdotool installed"]
    fn live_xkb_switches_and_shortcut_grabs() {
        let keyboard = Arc::new(Keyboard::connect().unwrap());
        let before = keyboard.layouts().unwrap();
        assert!(before.names.len() >= 2);
        keyboard.switch("next").unwrap();
        assert_eq!(
            keyboard.layouts().unwrap().current_idx,
            (before.current_idx + 1) % before.names.len()
        );
        keyboard.switch(&before.current_idx.to_string()).unwrap();
        assert!(keyboard.switch("99").is_err());
        let on = shortcut::Shortcut {
            key: "Ctrl+Alt+L".into(),
            enabled: true,
        };
        let off = shortcut::Shortcut {
            enabled: false,
            ..on.clone()
        };
        keyboard.set_shortcut(&on).unwrap();
        let competing = Keyboard::connect().unwrap();
        assert!(competing.set_shortcut(&on).is_err());
        let (sender, receiver) = std::sync::mpsc::channel();
        keyboard.watch(sender).unwrap();
        for locks in [false, true] {
            let args = if locks {
                vec!["key", "Caps_Lock", "Num_Lock", "ctrl+alt+l"]
            } else {
                vec!["key", "ctrl+alt+l"]
            };
            assert!(
                std::process::Command::new("xdotool")
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                let action = receiver
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .expect("shortcut did not fire");
                if matches!(action, Action::Next) {
                    break;
                }
            }
        }
        keyboard.set_shortcut(&off).unwrap();
        competing.set_shortcut(&on).unwrap();
        competing.set_shortcut(&off).unwrap();
    }
}
