use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    path::PathBuf,
};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct Profile {
    pub name: String,
    pub program: String,
    pub arguments: Vec<String>,
    pub directory: String,
}
impl Default for Profile {
    fn default() -> Self {
        #[cfg(target_os = "mochios")]
        let (program, arguments) = ("/system/bin/msh".into(), vec!["--login-tty".into()]);
        #[cfg(not(target_os = "mochios"))]
        let (program, arguments) = (
            std::env::var("SHELL")
                .ok()
                .filter(|s| s.starts_with('/'))
                .unwrap_or_else(|| "/bin/sh".into()),
            vec![],
        );
        Self {
            name: "Shell".into(),
            program,
            arguments,
            directory: String::new(),
        }
    }
}
impl Profile {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty()
            || self.name.len() > 80
            || self.name.chars().any(char::is_control)
        {
            return Err("Enter a profile name (up to 80 bytes).".into());
        }
        if !self.program.starts_with('/')
            || self.program.len() > 4096
            || self.program.contains('\0')
        {
            return Err("Use an absolute executable path.".into());
        }
        if self.arguments.iter().map(String::len).sum::<usize>() > 16_384
            || self.arguments.len() > 64
            || self
                .arguments
                .iter()
                .any(|a| a.len() > 4096 || a.contains('\0'))
        {
            return Err("Arguments are too long or contain NUL.".into());
        }
        if !self.directory.is_empty()
            && (!self.directory.starts_with('/')
                || self.directory.len() > 4096
                || self.directory.contains('\0'))
        {
            return Err("Use an absolute starting directory, or leave it empty to inherit.".into());
        }
        #[cfg(target_os = "mochios")]
        if !self.directory.is_empty() {
            return Err(
                "Starting directories currently inherit the native POSIX launch scope.".into(),
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct Settings {
    pub version: u32,
    pub profiles: Vec<Profile>,
    pub default_profile: usize,
    pub font_size: f32,
    pub scrollback: usize,
    pub palette: String,
    pub cursor: String,
    pub copy_on_select: bool,
    pub confirm_close: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            profiles: vec![Profile::default()],
            default_profile: 0,
            font_size: 13.0,
            scrollback: 2000,
            palette: "System".into(),
            cursor: "Bar".into(),
            copy_on_select: false,
            confirm_close: true,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported terminal settings version.".into());
        }
        if self.profiles.is_empty()
            || self.profiles.len() > 12
            || self.default_profile >= self.profiles.len()
        {
            return Err("Use 1–12 profiles and select a valid default profile.".into());
        }
        for p in &self.profiles {
            p.validate()?;
        }
        if !self.font_size.is_finite()
            || !(10.0..=32.0).contains(&self.font_size)
            || !(100..=10000).contains(&self.scrollback)
        {
            return Err("Text size must be 10–32 px and scrollback 100–10,000 lines.".into());
        }
        if !["System", "Light", "Dark"].contains(&self.palette.as_str())
            || !["Bar", "Block", "Underline"].contains(&self.cursor.as_str())
        {
            return Err("Choose a supported color scheme and cursor shape.".into());
        }
        Ok(())
    }
    pub fn path() -> PathBuf {
        #[cfg(target_os = "linux")]
        if let Some(root) = std::env::var_os("MOCHIOS_PREVIEW_CONFIG_ROOT") {
            return PathBuf::from(root).join("terminal/settings.toml");
        }
        #[cfg(target_os = "linux")]
        if let Some(base) = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        {
            return base.join("mochios/terminal/settings.toml");
        }
        let user: String = std::env::var("USER")
            .unwrap_or_else(|_| "default".into())
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "_.-".contains(*c))
            .take(80)
            .collect();
        PathBuf::from("/var/config/applications/org.mochios.terminal/users")
            .join(if user.is_empty() { "default" } else { &user })
            .join("settings.toml")
    }
    pub fn load() -> (Self, Option<String>) {
        match (|| -> io::Result<String> {
            let mut text = String::new();
            fs::File::open(Self::path())?
                .take(1_048_577)
                .read_to_string(&mut text)?;
            if text.len() > 1_048_576 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Settings exceed 1 MiB",
                ));
            }
            Ok(text)
        })() {
            Ok(text) => match toml::from_str::<Self>(&text)
                .map_err(|e| e.to_string())
                .and_then(|s| {
                    s.validate()?;
                    Ok(s)
                }) {
                Ok(s) => (s, None),
                Err(e) => (
                    Self::default(),
                    Some(format!("Settings could not be loaded: {e}")),
                ),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(format!("Settings could not be read: {e}")),
            ),
        }
    }
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        self.validate()?;
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let parent = path.parent().ok_or("Invalid settings path")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let temporary = path.with_extension(format!("toml.{}.new", std::process::id()));
        let mut created = false;
        let result = (|| -> io::Result<()> {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            created = true;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() && created {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|e| e.to_string())
    }
}
/// Literal argv parsing, with quotes but no shell expansion or evaluation.
pub(crate) fn arguments(text: &str) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut value = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for c in text.chars() {
        if escaped {
            value.push(c);
            escaped = false;
            started = true;
        } else if c == '\\' && quote != Some('\'') {
            escaped = true;
            started = true;
        } else if Some(c) == quote {
            quote = None;
        } else if quote.is_some() {
            value.push(c);
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            started = true;
        } else if c.is_whitespace() {
            if started {
                result.push(std::mem::take(&mut value));
                started = false;
            }
        } else {
            value.push(c);
            started = true;
        }
    }
    if escaped || quote.is_some() {
        return Err("Close the quote or finish the escaped argument.".into());
    }
    if started {
        result.push(value);
    }
    Ok(result)
}
pub(crate) fn format_arguments(args: &[String]) -> String {
    args.iter()
        .map(|a| format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(" ")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_arguments_round_trip() {
        let a = vec![
            "a b".into(),
            "$(do not run)".into(),
            "".into(),
            "x\\\"y".into(),
        ];
        assert_eq!(arguments(&format_arguments(&a)).unwrap(), a);
        assert!(arguments("'unterminated").is_err());
    }
    #[test]
    fn save_is_private_atomic_and_does_not_delete_an_existing_temp_file() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("terminal-settings-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.toml");
        let temp = path.with_extension(format!("toml.{}.new", std::process::id()));
        fs::write(&temp, "owned by someone else").unwrap();
        assert!(Settings::default().save_to(&path).is_err());
        assert_eq!(fs::read_to_string(&temp).unwrap(), "owned by someone else");
        fs::remove_file(temp).unwrap();
        Settings::default().save_to(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let parsed: Settings = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed, Settings::default());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_settings_rejected() {
        let mut s = Settings::default();
        s.font_size = f32::NAN;
        assert!(s.validate().is_err());
        s = Settings::default();
        s.profiles[0].program = "relative".into();
        assert!(s.validate().is_err());
        assert_eq!(
            toml::from_str::<Settings>(&toml::to_string(&Settings::default()).unwrap()).unwrap(),
            Settings::default()
        );
    }
}
