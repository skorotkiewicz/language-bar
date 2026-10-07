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

pub fn binding(key: &str) -> Result<String, &'static str> {
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
    Ok(format!(
        "binds {{\n    {key} {{ switch-layout \"next\"; }}\n}}\n"
    ))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temp, bytes)?;
    fs::rename(temp, path)
}

pub fn save(dir: &Path, shortcut: &Shortcut) -> Result<(), Box<dyn std::error::Error>> {
    let binding = binding(&shortcut.key)?;
    fs::create_dir_all(dir)?;
    // Validate even a disabled shortcut, so enabling it cannot introduce invalid syntax.
    let validation = dir.join("validate.kdl");
    fs::write(&validation, &binding)?;
    let output = Command::new("niri")
        .args(["validate", "--config"])
        .arg(&validation)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Invalid niri shortcut: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let config = if shortcut.enabled {
        binding.as_str()
    } else {
        "// Shortcut disabled. Enable it from the language tray menu.\n"
    };
    let path = dir.join("shortcut.kdl");
    let previous = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    atomic_write(&path, config.as_bytes())?;
    if let Err(error) = atomic_write(
        &dir.join("settings.json"),
        &serde_json::to_vec_pretty(shortcut)?,
    ) {
        // Keep the niri include and saved preferences consistent if the second write fails.
        atomic_write(&path, &previous)?;
        return Err(error.into());
    }
    Ok(())
}

pub fn instructions(dir: &Path) -> String {
    format!(
        "Add this line once to your niri config. Use the tray checkbox to enable or disable the shortcut:\n\ninclude {}\n\nNiri reloads it automatically. Avoid keys already bound in your config.",
        serde_json::to_string(&dir.join("shortcut.kdl").to_string_lossy()).unwrap()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_is_opt_in_and_cannot_inject_kdl() {
        assert!(!Shortcut::default().enabled);
        assert_eq!(
            binding("Ctrl+Alt+L").unwrap(),
            "binds {\n    Ctrl+Alt+L { switch-layout \"next\"; }\n}\n"
        );
        for key in [
            "",
            "Space",
            "Mod+",
            "Bogus+L",
            "Mod+Space; spawn evil",
            "Mod+L\n}",
        ] {
            assert!(binding(key).is_err(), "{key}");
        }
    }
}
