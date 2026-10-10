//! Brings Discord's own window to the front (the strip's server logo); the window search and the
//! raise are the shared `taskbar_dock::window`.

use taskbar_dock::window;

/// Discord's programs: stable, PTB and Canary.
const PROGRAMS: [&str; 3] = ["discord.exe", "discordptb.exe", "discordcanary.exe"];

fn is_discord(path: &str) -> bool {
    PROGRAMS.iter().any(|exe| window::is_exe(path, exe))
}

/// Raises Discord's main window. With none open (Discord sits in the tray), the `discord://`
/// link makes the running Discord show itself.
pub(crate) fn open_discord() {
    window::raise(&|h| window::exe_path(h).is_some_and(|p| is_discord(&p)), "discord://-/channels/@me");
}

#[cfg(test)]
mod tests {
    use super::is_discord;

    #[test]
    fn finds_discord_programs() {
        assert!(is_discord(r"C:\Users\me\AppData\Local\Discord\app-1.0.9\Discord.exe"));
        assert!(is_discord(r"C:\x\DiscordPTB.exe"));
        assert!(!is_discord(r"C:\Program Files\Mozilla Firefox\firefox.exe"));
        assert!(!is_discord(r"C:\x\Update.exe"));
    }
}
