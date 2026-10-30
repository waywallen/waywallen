use clap::{ArgAction, Parser};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "waywallen", version)]
pub struct DaemonConfig {
    #[arg(long, value_name = "PORT", default_value_t = 0)]
    pub ws_port: u16,

    #[arg(long = "ui", value_name = "PATH")]
    pub ui_path: Option<PathBuf>,

    #[arg(long)]
    pub no_ui: bool,

    #[arg(long)]
    pub no_tray: bool,

    #[arg(long = "plugin", value_name = "PATH")]
    pub plugin_dirs: Vec<PathBuf>,

    #[arg(
        long,
        value_name = "NAME",
        help = "Display backend name [built-ins: kde-plasma, gnome-shell, layer-shell, x11]"
    )]
    pub display_backend: Option<String>,

    #[arg(long)]
    pub no_display: bool,

    #[arg(long = "no-restore", action = ArgAction::SetFalse)]
    pub restore_last: bool,
}

impl DaemonConfig {
    pub fn from_env() -> Self {
        Self::parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_builtin_display_backends() {
        let error = DaemonConfig::try_parse_from(["waywallen", "--help"]).unwrap_err();
        let help = error.to_string();

        assert!(help.contains("kde-plasma, gnome-shell, layer-shell"));
    }
}
