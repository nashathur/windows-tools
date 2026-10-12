//! The right-click panel's own rows (Block adds Show on taskbar, Compact mode, Start with
//! Windows, Edit settings… and Quit) and following edits to the settings file.

use super::*;
use taskbar_dock::menu_panel::{glyph, Row};

const ID_REFRESH: usize = 100;
const ID_FAHRENHEIT: usize = 103;

/// Header: the location and a one-line summary; then Refresh now, Fahrenheit (Block's switches
/// join it).
pub(crate) fn build(a: &App, m: &mut MenuBuilder) {
    let c = a.full_content();
    let title = a.weather.as_ref().map_or("Weather".to_string(), |w| w.location.clone());
    let subtitle = [c.line1, c.line2, c.desc].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
    m.title(title)
        .subtitle(subtitle)
        .item(Row::action(ID_REFRESH, glyph::REFRESH, "Refresh now"))
        .sep()
        .item(Row::switch(ID_FAHRENHEIT, "Fahrenheit (°F)", a.cfg.fahrenheit()));
}

/// A switch flipped (the panel stays open): its new state.
pub(crate) fn switch(a: &mut App, id: usize, _ctx: &mut Ctx) -> bool {
    match id {
        ID_FAHRENHEIT => {
            a.cfg.units = if a.cfg.fahrenheit() { "celsius" } else { "fahrenheit" }.into();
            let _ = taskbar_dock::settings::set_key("weather", "units", serde_json::json!(a.cfg.units));
            fetch::start_fetch(a);
            a.cfg.fahrenheit()
        }
        _ => false,
    }
}

pub(crate) fn action(a: &mut App, id: usize, _ctx: &mut Ctx) {
    if id == ID_REFRESH {
        a.cfg = Config::read();
        fetch::restart(a);
    }
}

/// Picks up edits to the shared settings file (Edit settings…, another widget's write). Act only
/// on what changed: the weather data settings → fetch again; the position → redraw (Block owns
/// `compact` and `show_on_taskbar`).
pub(crate) fn settings_changed(a: &mut App, ctx: &mut Ctx) {
    let cfg = Config::read();
    let old = std::mem::replace(&mut a.cfg, cfg.clone());
    let data = |c: &Config| {
        (c.city.clone(), c.latitude, c.longitude, c.location_name.clone(), c.units.clone(),
         c.refresh_minutes, c.model.clone(), c.knmi_api_key.clone(), c.knmi_station.clone())
    };
    if old.label_position != cfg.label_position {
        ctx.redraw();
    }
    if data(&old) != data(&cfg) {
        if old.location_key() != cfg.location_key() {
            fetch::restart(a);
        } else {
            fetch::start_fetch(a);
        }
    }
}
