//! The tray icon's logo and tooltip. Block shows the icon whenever the strip isn't (Show on
//! taskbar off) and handles its clicks; weather only supplies these two.

use super::*;

/// The tooltip: the current weather in a few lines (the icon itself is a static logo).
pub(crate) fn tip(a: &App) -> String {
    match &a.weather {
        Some(wx) => {
            let (_, desc) = describe(wx.code, wx.is_day);
            let mut tip = format!(
                "{}{} {} – {}\nFeels like {} · Humidity {:.0}% · Wind {:.0} {}",
                deg(wx.temp), &wx.unit()[2..], desc, wx.location,
                deg(wx.feels), wx.humidity, wx.wind, wx.wind_unit()
            );
            match (&wx.observed, &wx.obs_error) {
                (Some(o), _) => tip.push_str(&format!("\nMeasured: KNMI {o}")),
                (None, Some(e)) => tip.push_str(&format!("\n{e}")),
                _ => {}
            }
            if let Some(e) = &a.error {
                tip.push_str(&format!("\nUpdate failed: {e}"));
            }
            tip
        }
        None => match &a.error {
            Some(e) => format!("Weather unavailable: {e}"),
            None => "Loading weather…".to_string(),
        },
    }
}
