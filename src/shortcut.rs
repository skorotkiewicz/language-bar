use crate::backend::Kind;
use serde::{Deserialize, Serialize};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shortcut {
    pub key: String,
    pub enabled: bool,
}

impl Default for Shortcut {
    fn default() -> Self {
        Self {
            key: "Mod+Space".into(),
            enabled: false,
        }
    }
}

pub fn directory() -> io::Result<PathBuf> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|s| PathBuf::from(s).join(".config")))
        .ok_or_else(|| io::Error::other("Set HOME or XDG_CONFIG_HOME"))?;
    Ok(base.join("language-tray-indicator"))
}

pub fn load(dir: &Path) -> Result<Shortcut, Box<dyn std::error::Error>> {
    match fs::read(dir.join("settings.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Shortcut::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn parts(key: &str) -> Result<(Vec<&str>, &str), &'static str> {
    // Only one key with optional modifiers, never arbitrary KDL or a shell command.
    let parts: Vec<_> = key.split('+').collect();
    let Some(last) = parts.last() else {
        return Err("Enter a shortcut");
    };
    if parts.len() < 2
        || parts.len() > 5
        || !parts[..parts.len() - 1]
            .iter()
            .all(|p| matches!(*p, "Mod" | "Super" | "Ctrl" | "Alt" | "Shift"))
        || last.is_empty()
        || !last.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("Use modifiers and a key, for example Mod+Space or Ctrl+Alt+L");
    }
    Ok((parts[..parts.len() - 1].to_vec(), last))
}

pub fn binding(key: &str, kind: Kind) -> Result<String, &'static str> {
    let (modifiers, name) = parts(key)?;
    if kind == Kind::Niri {
        return Ok(format!(
            "binds {{\n    {key} {{ switch-layout \"next\"; }}\n}}\n"
        ));
    }
    let symbol = xkbcommon::xkb::keysym_from_name(name, xkbcommon::xkb::KEYSYM_CASE_INSENSITIVE);
    if symbol.raw() == 0 {
        return Err("Unknown XKB key name");
    }
    if kind == Kind::X11 {
        return Ok(String::new());
    }
    let mut parts: Vec<_> = modifiers
        .iter()
        .map(|m| match *m {
            "Mod" | "Super" => "Mod4".to_string(),
            "Ctrl" => "Control".into(),
            "Alt" => "Mod1".into(),
            _ => (*m).into(),
        })
        .collect();
    parts.push(xkbcommon::xkb::keysym_get_name(symbol));
    Ok(format!(
        "bindsym {} input type:keyboard xkb_switch_layout next\n",
        parts.join("+")
    ))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temp, bytes)?;
    fs::rename(temp, path)
}

pub fn save(dir: &Path, shortcut: &Shortcut, kind: Kind) -> Result<(), Box<dyn std::error::Error>> {
    let binding = binding(&shortcut.key, kind)?;
    fs::create_dir_all(dir)?;
    let settings = serde_json::to_vec_pretty(shortcut)?;
    if kind == Kind::X11 {
        atomic_write(&dir.join("settings.json"), &settings)?;
        return Ok(());
    }
    let suffix = if kind == Kind::Niri { "kdl" } else { "conf" };
    // Validate even a disabled shortcut, so enabling it cannot introduce invalid syntax.
    let validation = dir.join(format!("validate.{suffix}"));
    fs::write(&validation, &binding)?;
    let output = if kind == Kind::Niri {
        Command::new("niri")
            .args(["validate", "--config"])
            .arg(&validation)
            .output()?
    } else {
        Command::new("sway")
            .args(["--validate", "--config"])
            .arg(&validation)
            .output()?
    };
    if !output.status.success() {
        return Err(format!(
            "Invalid shortcut: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let config = if shortcut.enabled {
        binding.as_str()
    } else if kind == Kind::Niri {
        "// Shortcut disabled. Enable it from the language tray menu.\n"
    } else {
        "# Shortcut disabled. Enable it from the language tray menu.\n"
    };
    let path = dir.join(format!("shortcut.{suffix}"));
    let previous = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    atomic_write(&path, config.as_bytes())?;
    if let Err(error) = atomic_write(&dir.join("settings.json"), &settings) {
        // Keep the niri include and saved preferences consistent if the second write fails.
        atomic_write(&path, &previous)?;
        return Err(error.into());
    }
    Ok(())
}

pub fn instructions(dir: &Path, kind: Kind) -> String {
    if kind == Kind::X11 {
        return "X11 shortcuts register directly while the tray runs. Mod means Super. CLI changes apply on the next tray start. Avoid keys already registered by another app.".into();
    }
    let (desktop, suffix, reload) = if kind == Kind::Niri {
        ("niri", "kdl", "Niri reloads it automatically.")
    } else {
        (
            "Sway",
            "conf",
            "Reload Sway after adding the include. Tray shortcut changes reload Sway automatically.",
        )
    };
    format!(
        "Add this line once to your {desktop} config. Use the tray checkbox to enable or disable the shortcut:\n\ninclude {}\n\n{reload} Avoid keys already bound in your config.",
        serde_json::to_string(&dir.join(format!("shortcut.{suffix}")).to_string_lossy()).unwrap()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_is_opt_in_and_cannot_inject_kdl() {
        assert!(!Shortcut::default().enabled);
        assert_eq!(
            binding("Ctrl+Alt+L", Kind::Niri).unwrap(),
            "binds {\n    Ctrl+Alt+L { switch-layout \"next\"; }\n}\n"
        );
        assert_eq!(
            binding("Ctrl+Alt+L", Kind::Sway).unwrap(),
            "bindsym Control+Mod1+l input type:keyboard xkb_switch_layout next\n"
        );
        assert!(binding("Mod+Space", Kind::X11).is_ok());
        assert!(binding("Mod+NotAKey", Kind::Sway).is_err());
        for key in [
            "",
            "Space",
            "Mod+",
            "Bogus+L",
            "Mod+Space; spawn evil",
            "Mod+L\n}",
        ] {
            assert!(binding(key, Kind::Niri).is_err(), "{key}");
        }
    }
}
